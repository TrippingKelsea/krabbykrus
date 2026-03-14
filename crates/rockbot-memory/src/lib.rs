//! Memory management system for RockBot
//!
//! Provides document storage, keyword search, TF-IDF vector semantic search,
//! and optional embedding-based hybrid search when an embedding provider is available.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;

/// Memory system errors
#[derive(Debug, Error)]
pub enum MemoryError {
    #[error("Memory store not found: {store_id}")]
    StoreNotFound { store_id: String },

    #[error("Search failed: {message}")]
    SearchFailed { message: String },

    #[error("Index error: {message}")]
    IndexError { message: String },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("System time error: {0}")]
    SystemTime(#[from] std::time::SystemTimeError),
}

/// Result type for memory operations
pub type Result<T> = std::result::Result<T, MemoryError>;

/// Trait for providing text embeddings (e.g. from an LLM API).
///
/// When set on a [`MemoryManager`], enables hybrid search that combines
/// TF-IDF scoring with dense embedding cosine similarity.
#[async_trait::async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Generate a dense embedding vector for the given text.
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;
}

/// Memory manager handles file-based memory storage and vector search
pub struct MemoryManager {
    workspace_path: PathBuf,
    documents: tokio::sync::RwLock<HashMap<String, MemoryDocument>>,
    vector_index: tokio::sync::RwLock<VectorIndex>,
    /// Optional embedding provider for hybrid search
    embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
}

/// A document stored in memory
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryDocument {
    pub id: String,
    pub path: String,
    pub content: String,
    pub chunks: Vec<MemoryChunk>,
    pub metadata: HashMap<String, serde_json::Value>,
    pub last_modified: DateTime<Utc>,
}

/// A chunk of a document for vector search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryChunk {
    pub id: String,
    pub document_id: String,
    pub content: String,
    pub start_offset: usize,
    pub end_offset: usize,
    pub metadata: HashMap<String, serde_json::Value>,
}

/// TF-IDF based vector index for semantic search
#[derive(Debug)]
struct VectorIndex {
    /// TF-IDF vectors keyed by chunk ID
    vectors: HashMap<String, SparseVector>,
    /// Dense embedding vectors keyed by chunk ID (populated when embedding provider available)
    embeddings: HashMap<String, DenseVector>,
    /// Chunk data keyed by chunk ID
    chunks: HashMap<String, MemoryChunk>,
    /// Document frequency: how many chunks contain each term
    doc_freq: HashMap<String, usize>,
    /// Total number of indexed chunks
    total_chunks: usize,
    /// Whether the index needs rebuilding
    dirty: bool,
}

/// Dense embedding vector for cosine similarity
#[derive(Debug, Clone)]
struct DenseVector {
    values: Vec<f32>,
    norm: f32,
}

impl DenseVector {
    fn new(values: Vec<f32>) -> Self {
        let norm = values.iter().map(|v| v * v).sum::<f32>().sqrt();
        Self { values, norm }
    }

    fn cosine_similarity(&self, other: &DenseVector) -> f32 {
        if self.norm == 0.0 || other.norm == 0.0 || self.values.len() != other.values.len() {
            return 0.0;
        }
        let dot: f32 = self.values.iter().zip(&other.values).map(|(a, b)| a * b).sum();
        dot / (self.norm * other.norm)
    }
}

/// Sparse TF-IDF vector (only stores non-zero entries)
#[derive(Debug, Clone)]
struct SparseVector {
    /// (term, tfidf_weight) pairs sorted by term for efficient dot product
    entries: Vec<(String, f32)>,
    /// Precomputed L2 norm for cosine similarity
    norm: f32,
}

impl SparseVector {
    fn from_term_freqs(tf: &HashMap<String, usize>, doc_freq: &HashMap<String, usize>, total_docs: usize) -> Self {
        let mut entries: Vec<(String, f32)> = tf.iter()
            .filter_map(|(term, &count)| {
                let df = doc_freq.get(term).copied().unwrap_or(1) as f32;
                let idf = ((total_docs as f32) / df).ln() + 1.0;
                let tfidf = (count as f32) * idf;
                if tfidf > 0.0 {
                    Some((term.clone(), tfidf))
                } else {
                    None
                }
            })
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));

        let norm = entries.iter().map(|(_, w)| w * w).sum::<f32>().sqrt();

        Self { entries, norm }
    }

    fn cosine_similarity(&self, other: &SparseVector) -> f32 {
        if self.norm == 0.0 || other.norm == 0.0 {
            return 0.0;
        }

        // Merge-join on sorted term keys
        let mut dot = 0.0_f32;
        let mut i = 0;
        let mut j = 0;
        while i < self.entries.len() && j < other.entries.len() {
            match self.entries[i].0.cmp(&other.entries[j].0) {
                std::cmp::Ordering::Equal => {
                    dot += self.entries[i].1 * other.entries[j].1;
                    i += 1;
                    j += 1;
                }
                std::cmp::Ordering::Less => i += 1,
                std::cmp::Ordering::Greater => j += 1,
            }
        }

        dot / (self.norm * other.norm)
    }
}

/// Memory search query
#[derive(Debug, Clone)]
pub struct MemoryQuery {
    pub query: String,
    pub limit: Option<usize>,
    pub filters: HashMap<String, serde_json::Value>,
    pub semantic: bool,
}

/// Memory search result
#[derive(Debug, Clone)]
pub struct MemoryResult {
    pub chunks: Vec<SearchResultChunk>,
    pub total_results: usize,
}

/// A search result chunk
#[derive(Debug, Clone)]
pub struct SearchResultChunk {
    pub chunk: MemoryChunk,
    pub score: f32,
    pub highlights: Vec<String>,
}

// ---------------------------------------------------------------------------
// Tokenization
// ---------------------------------------------------------------------------

/// Common English stop words to exclude from TF-IDF indexing
const STOP_WORDS: &[&str] = &[
    "a", "an", "the", "and", "or", "but", "in", "on", "at", "to", "for",
    "of", "with", "by", "from", "is", "are", "was", "were", "be", "been",
    "being", "have", "has", "had", "do", "does", "did", "will", "would",
    "could", "should", "may", "might", "can", "shall", "not", "no", "nor",
    "so", "if", "then", "than", "too", "very", "just", "about", "above",
    "after", "again", "all", "also", "am", "as", "because", "before",
    "between", "both", "each", "few", "further", "get", "got", "here",
    "how", "into", "it", "its", "more", "most", "much", "must", "my",
    "new", "now", "only", "other", "our", "out", "own", "same", "she",
    "he", "his", "her", "some", "such", "that", "their", "them", "there",
    "these", "they", "this", "those", "through", "up", "us", "we", "what",
    "when", "where", "which", "while", "who", "whom", "why", "you", "your",
];

/// Tokenize text into lowercase terms, filtering stop words and short tokens
fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_')
        .map(str::to_lowercase)
        .filter(|w| w.len() >= 2 && !STOP_WORDS.contains(&w.as_str()))
        .collect()
}

/// Build term frequency map from text
fn term_frequencies(text: &str) -> HashMap<String, usize> {
    let mut tf: HashMap<String, usize> = HashMap::new();
    for token in tokenize(text) {
        *tf.entry(token).or_insert(0) += 1;
    }
    tf
}

impl MemoryManager {
    /// Create a new memory manager
    pub async fn new(workspace_path: PathBuf) -> Result<Self> {
        tokio::fs::create_dir_all(&workspace_path).await?;

        let memory_dir = workspace_path.join("memory");
        tokio::fs::create_dir_all(&memory_dir).await?;

        Ok(Self {
            workspace_path,
            documents: tokio::sync::RwLock::new(HashMap::new()),
            vector_index: tokio::sync::RwLock::new(VectorIndex {
                vectors: HashMap::new(),
                embeddings: HashMap::new(),
                chunks: HashMap::new(),
                doc_freq: HashMap::new(),
                total_chunks: 0,
                dirty: false,
            }),
            embedding_provider: None,
        })
    }

    /// Set an embedding provider for hybrid search.
    /// When set, search results combine TF-IDF and embedding similarity scores.
    pub fn set_embedding_provider(&mut self, provider: Arc<dyn EmbeddingProvider>) {
        self.embedding_provider = Some(provider);
    }

    /// Load memory files from the workspace
    pub async fn load_memory_files(&self) -> Result<()> {
        let memory_dir = self.workspace_path.join("memory");

        // Load core memory files
        let memory_files = vec![
            "core.json",
            "continuity.md",
            "reflections.json",
            "../MEMORY.md",
            "../SOUL.md",
            "../USER.md",
        ];

        for file_name in memory_files {
            let file_path = memory_dir.join(file_name);
            if file_path.exists() {
                self.load_document(&file_path).await?;
            }
        }

        // Load daily logs
        let current_date = Utc::now().format("%Y-%m-%d").to_string();
        let yesterday = (Utc::now() - chrono::Duration::days(1)).format("%Y-%m-%d").to_string();

        for date in [current_date, yesterday] {
            let daily_log = memory_dir.join(format!("{date}.md"));
            if daily_log.exists() {
                self.load_document(&daily_log).await?;
            }
        }

        // Build the vector index after loading all documents
        self.rebuild_index().await?;

        Ok(())
    }

    /// Load a single document
    async fn load_document(&self, file_path: &PathBuf) -> Result<()> {
        let content = tokio::fs::read_to_string(file_path).await?;
        let metadata = tokio::fs::metadata(file_path).await?;

        let document_id = file_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        // Create chunks from content
        let chunks = self.create_chunks(&document_id, &content);

        let document = MemoryDocument {
            id: document_id.clone(),
            path: file_path.to_string_lossy().to_string(),
            content,
            chunks,
            metadata: HashMap::new(),
            last_modified: DateTime::from_timestamp(
                metadata.modified()?.duration_since(std::time::UNIX_EPOCH)?.as_secs() as i64,
                0,
            ).unwrap_or_else(Utc::now),
        };

        // Store document
        let mut documents = self.documents.write().await;
        documents.insert(document_id, document);

        // Mark index as needing rebuild
        let mut idx = self.vector_index.write().await;
        idx.dirty = true;

        Ok(())
    }

    /// Create chunks from document content (overlapping windows for better recall)
    #[allow(clippy::unused_self)]
    fn create_chunks(&self, document_id: &str, content: &str) -> Vec<MemoryChunk> {
        let mut chunks = Vec::new();
        let lines: Vec<&str> = content.lines().collect();
        let chunk_size: usize = 10; // Lines per chunk
        let overlap: usize = 3;     // Lines of overlap between chunks
        let step = chunk_size.saturating_sub(overlap).max(1);

        let mut i = 0;
        let mut chunk_idx = 0;
        while i < lines.len() {
            let end = (i + chunk_size).min(lines.len());
            let chunk_lines = &lines[i..end];
            let chunk_content = chunk_lines.join("\n");

            let chunk = MemoryChunk {
                id: format!("{document_id}:chunk:{chunk_idx}"),
                document_id: document_id.to_string(),
                content: chunk_content,
                start_offset: i,
                end_offset: end,
                metadata: HashMap::new(),
            };

            chunks.push(chunk);
            chunk_idx += 1;
            i += step;
        }

        chunks
    }

    /// Rebuild the TF-IDF vector index from all loaded documents
    async fn rebuild_index(&self) -> Result<()> {
        let documents = self.documents.read().await;

        // Phase 1: Collect all chunks and compute term frequencies
        let mut all_chunks: Vec<(String, MemoryChunk, HashMap<String, usize>)> = Vec::new();
        let mut global_doc_freq: HashMap<String, usize> = HashMap::new();

        for document in documents.values() {
            for chunk in &document.chunks {
                let tf = term_frequencies(&chunk.content);
                // Update document frequency (each term counted once per chunk)
                for term in tf.keys() {
                    *global_doc_freq.entry(term.clone()).or_insert(0) += 1;
                }
                all_chunks.push((chunk.id.clone(), chunk.clone(), tf));
            }
        }

        let total_chunks = all_chunks.len();

        // Phase 2: Build TF-IDF vectors
        let mut vectors: HashMap<String, SparseVector> = HashMap::with_capacity(total_chunks);
        let mut chunk_map: HashMap<String, MemoryChunk> = HashMap::with_capacity(total_chunks);

        for (chunk_id, chunk, tf) in all_chunks {
            let vec = SparseVector::from_term_freqs(&tf, &global_doc_freq, total_chunks);
            vectors.insert(chunk_id.clone(), vec);
            chunk_map.insert(chunk_id, chunk);
        }

        // Phase 3: Optionally compute dense embeddings
        let mut embeddings: HashMap<String, DenseVector> = HashMap::new();
        if let Some(ref provider) = self.embedding_provider {
            for (chunk_id, chunk) in &chunk_map {
                match provider.embed(&chunk.content).await {
                    Ok(vec) => {
                        embeddings.insert(chunk_id.clone(), DenseVector::new(vec));
                    }
                    Err(e) => {
                        tracing::warn!("Failed to embed chunk {chunk_id}: {e}");
                    }
                }
            }
            tracing::debug!("Computed {} embeddings for {} chunks", embeddings.len(), total_chunks);
        }

        // Phase 4: Store the index
        let mut idx = self.vector_index.write().await;
        idx.vectors = vectors;
        idx.embeddings = embeddings;
        idx.chunks = chunk_map;
        idx.doc_freq = global_doc_freq;
        idx.total_chunks = total_chunks;
        idx.dirty = false;

        tracing::debug!("Rebuilt TF-IDF index: {} chunks indexed", total_chunks);
        Ok(())
    }

    /// Search memory
    pub async fn search(&self, query: MemoryQuery) -> Result<MemoryResult> {
        // Rebuild index if dirty
        {
            let idx = self.vector_index.read().await;
            if idx.dirty {
                drop(idx);
                self.rebuild_index().await?;
            }
        }

        if query.semantic {
            self.semantic_search(query).await
        } else {
            self.keyword_search(query).await
        }
    }

    /// Perform keyword search
    async fn keyword_search(&self, query: MemoryQuery) -> Result<MemoryResult> {
        let documents = self.documents.read().await;
        let mut results = Vec::new();
        let query_lower = query.query.to_lowercase();

        for document in documents.values() {
            for chunk in &document.chunks {
                let content_lower = chunk.content.to_lowercase();
                if content_lower.contains(&query_lower) {
                    // Simple relevance scoring based on frequency and position
                    let matches = content_lower.matches(&query_lower).count();
                    let score = matches as f32 / chunk.content.len() as f32 * 1000.0;

                    // Create highlights
                    let highlights = vec![self.create_highlight(&chunk.content, &query.query)];

                    results.push(SearchResultChunk {
                        chunk: chunk.clone(),
                        score,
                        highlights,
                    });
                }
            }
        }

        // Sort by relevance score
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

        // Apply limit
        let limit = query.limit.unwrap_or(10);
        let total_results = results.len();
        results.truncate(limit);

        Ok(MemoryResult {
            chunks: results,
            total_results,
        })
    }

    /// Perform TF-IDF cosine similarity search with optional embedding hybrid scoring.
    ///
    /// When an embedding provider is available and embeddings have been computed,
    /// the final score is `0.3 * tfidf_score + 0.7 * embedding_score`.
    /// Otherwise, falls back to pure TF-IDF scoring.
    async fn semantic_search(&self, query: MemoryQuery) -> Result<MemoryResult> {
        let idx = self.vector_index.read().await;

        if idx.total_chunks == 0 {
            return Ok(MemoryResult { chunks: Vec::new(), total_results: 0 });
        }

        // Tokenize query and build its TF-IDF vector using the index's doc frequencies
        let query_tf = term_frequencies(&query.query);
        let query_vec = SparseVector::from_term_freqs(&query_tf, &idx.doc_freq, idx.total_chunks);

        if query_vec.norm == 0.0 {
            // No meaningful terms in query — fall back to keyword search
            drop(idx);
            return self.keyword_search(query).await;
        }

        // Compute query embedding if provider is available and index has embeddings
        let query_embedding = if !idx.embeddings.is_empty() {
            if let Some(ref provider) = self.embedding_provider {
                match provider.embed(&query.query).await {
                    Ok(vec) => Some(DenseVector::new(vec)),
                    Err(e) => {
                        tracing::warn!("Failed to embed query: {e}");
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        let use_hybrid = query_embedding.is_some();

        // Score every chunk
        let mut scored: Vec<(String, f32)> = idx.vectors.iter()
            .map(|(chunk_id, chunk_vec)| {
                let tfidf_sim = query_vec.cosine_similarity(chunk_vec);

                let score = if use_hybrid {
                    let emb_sim = query_embedding.as_ref()
                        .and_then(|qe| idx.embeddings.get(chunk_id).map(|ce| qe.cosine_similarity(ce)))
                        .unwrap_or(0.0);
                    // Hybrid: weight embeddings higher since they capture semantic similarity
                    0.3 * tfidf_sim + 0.7 * emb_sim
                } else {
                    tfidf_sim
                };

                (chunk_id.clone(), score)
            })
            .filter(|(_, sim)| *sim > 0.01)
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let limit = query.limit.unwrap_or(10);
        let total_results = scored.len();
        scored.truncate(limit);

        let results: Vec<SearchResultChunk> = scored.into_iter()
            .filter_map(|(chunk_id, score)| {
                let chunk = idx.chunks.get(&chunk_id)?.clone();
                let highlights = vec![self.create_highlight(&chunk.content, &query.query)];
                Some(SearchResultChunk { chunk, score, highlights })
            })
            .collect();

        Ok(MemoryResult { chunks: results, total_results })
    }

    /// Create a highlight snippet
    #[allow(clippy::unused_self)]
    fn create_highlight(&self, content: &str, query: &str) -> String {
        let query_lower = query.to_lowercase();
        let content_lower = content.to_lowercase();

        if let Some(pos) = content_lower.find(&query_lower) {
            let start = pos.saturating_sub(50);
            let end = (pos + query.len() + 50).min(content.len());
            let snippet = &content[start..end];

            // Replace query with highlighted version
            snippet.replace(query, &format!("**{query}**"))
        } else {
            // For semantic search, there may not be an exact match — show the start
            content.chars().take(100).collect()
        }
    }

    /// Write to daily log
    pub async fn write_daily_log(&self, content: &str) -> Result<()> {
        let date = Utc::now().format("%Y-%m-%d").to_string();
        let log_path = self.workspace_path.join("memory").join(format!("{date}.md"));

        // Append to daily log
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .await?;

        use tokio::io::AsyncWriteExt;
        file.write_all(content.as_bytes()).await?;
        file.write_all(b"\n").await?;

        // Reload the document
        self.load_document(&log_path).await?;

        Ok(())
    }

    /// Update core memory
    pub async fn update_core_memory(&self, key: &str, value: serde_json::Value) -> Result<()> {
        let core_path = self.workspace_path.join("memory").join("core.json");

        // Load existing core memory
        let mut core_data: HashMap<String, serde_json::Value> = if core_path.exists() {
            let content = tokio::fs::read_to_string(&core_path).await?;
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            HashMap::new()
        };

        // Update value
        core_data.insert(key.to_string(), value);

        // Write back to file
        let json_content = serde_json::to_string_pretty(&core_data)?;
        tokio::fs::write(&core_path, json_content).await?;

        // Reload document
        self.load_document(&core_path).await?;

        Ok(())
    }

    /// Get statistics about the vector index
    pub async fn index_stats(&self) -> (usize, usize) {
        let idx = self.vector_index.read().await;
        (idx.total_chunks, idx.doc_freq.len())
    }
}

/// Default vector index implementation
impl Default for VectorIndex {
    fn default() -> Self {
        Self {
            vectors: HashMap::new(),
            embeddings: HashMap::new(),
            chunks: HashMap::new(),
            doc_freq: HashMap::new(),
            total_chunks: 0,
            dirty: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Episodic Memory — cross-session interaction recall
// ---------------------------------------------------------------------------

/// A single episode (summary of a past agent interaction).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Episode {
    /// Session this episode came from.
    pub session_id: String,
    /// When the interaction occurred.
    pub timestamp: DateTime<Utc>,
    /// LLM-generated summary of the interaction.
    pub summary: String,
    /// Outcome: "success", "partial", "error".
    pub outcome: String,
    /// Tools that were used.
    pub tools_used: Vec<String>,
    /// Token count for the interaction.
    pub tokens_used: u64,
}

/// File-based episodic memory store.
///
/// Episodes are stored as JSONL files per agent in the agent's data directory.
/// Recall uses keyword matching against episode summaries (future: embedding search).
pub struct EpisodicStore {
    base_path: PathBuf,
}

impl EpisodicStore {
    /// Create an episodic store rooted at `base_path`.
    /// Episodes for agent "foo" are stored at `base_path/foo/episodes.jsonl`.
    pub fn new(base_path: PathBuf) -> Self {
        Self { base_path }
    }

    /// Store a new episode for the given agent.
    pub async fn store(&self, agent_id: &str, episode: &Episode) -> Result<()> {
        let dir = self.base_path.join(agent_id);
        tokio::fs::create_dir_all(&dir).await?;
        let file_path = dir.join("episodes.jsonl");

        let line = serde_json::to_string(episode)?;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)
            .await?;
        use tokio::io::AsyncWriteExt;
        file.write_all(line.as_bytes()).await?;
        file.write_all(b"\n").await?;
        Ok(())
    }

    /// Recall the most relevant episodes for a query.
    ///
    /// Uses simple keyword matching against summaries. Returns up to `limit` episodes
    /// sorted by relevance (most recent wins on ties).
    pub async fn recall(&self, agent_id: &str, query: &str, limit: usize) -> Result<Vec<Episode>> {
        let file_path = self.base_path.join(agent_id).join("episodes.jsonl");
        if !file_path.exists() {
            return Ok(Vec::new());
        }

        let content = tokio::fs::read_to_string(&file_path).await?;
        let query_lower = query.to_lowercase();
        let query_terms: Vec<&str> = query_lower.split_whitespace().collect();

        let mut scored: Vec<(f32, Episode)> = content.lines()
            .filter_map(|line| serde_json::from_str::<Episode>(line).ok())
            .map(|ep| {
                let summary_lower = ep.summary.to_lowercase();
                let term_hits = query_terms.iter()
                    .filter(|t| summary_lower.contains(**t))
                    .count();
                let score = term_hits as f32 / query_terms.len().max(1) as f32;
                (score, ep)
            })
            .filter(|(score, _)| *score > 0.0)
            .collect();

        // Sort by score descending, then by timestamp descending (most recent first)
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.1.timestamp.cmp(&a.1.timestamp))
        });

        scored.truncate(limit);
        Ok(scored.into_iter().map(|(_, ep)| ep).collect())
    }

    /// Get the total number of stored episodes for an agent.
    pub async fn episode_count(&self, agent_id: &str) -> Result<usize> {
        let file_path = self.base_path.join(agent_id).join("episodes.jsonl");
        if !file_path.exists() {
            return Ok(0);
        }
        let content = tokio::fs::read_to_string(&file_path).await?;
        Ok(content.lines().filter(|l| !l.is_empty()).count())
    }
}

/// Mock memory manager for testing
pub struct MockMemoryManager;

impl Default for MockMemoryManager {
    fn default() -> Self {
        Self::new()
    }
}

impl MockMemoryManager {
    pub fn new() -> Self {
        Self
    }

    pub async fn search(&self, _query: MemoryQuery) -> Result<MemoryResult> {
        Ok(MemoryResult {
            chunks: Vec::new(),
            total_results: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_memory_manager_creation() {
        let temp_dir = TempDir::new().unwrap();
        let memory_manager = MemoryManager::new(temp_dir.path().to_path_buf()).await.unwrap();

        // Should create memory directory
        assert!(temp_dir.path().join("memory").exists());

        let (chunks, terms) = memory_manager.index_stats().await;
        assert_eq!(chunks, 0);
        assert_eq!(terms, 0);
    }

    #[tokio::test]
    async fn test_keyword_search() {
        let temp_dir = TempDir::new().unwrap();
        let memory_manager = MemoryManager::new(temp_dir.path().to_path_buf()).await.unwrap();

        // Create a test file
        let test_file = temp_dir.path().join("memory").join("test.md");
        tokio::fs::write(&test_file, "This is a test document with some content").await.unwrap();

        // Load the document
        memory_manager.load_document(&test_file).await.unwrap();

        // Search for content
        let query = MemoryQuery {
            query: "test".to_string(),
            limit: Some(5),
            filters: HashMap::new(),
            semantic: false,
        };

        let results = memory_manager.search(query).await.unwrap();
        assert!(results.total_results > 0);
    }

    #[tokio::test]
    async fn test_semantic_search_basic() {
        let temp_dir = TempDir::new().unwrap();
        let mm = MemoryManager::new(temp_dir.path().to_path_buf()).await.unwrap();

        // Create documents with distinct topics
        let file1 = temp_dir.path().join("memory").join("rust.md");
        tokio::fs::write(&file1, "Rust programming language memory safety ownership borrowing compiler").await.unwrap();

        let file2 = temp_dir.path().join("memory").join("cooking.md");
        tokio::fs::write(&file2, "Recipe for chocolate cake flour sugar eggs butter baking oven temperature").await.unwrap();

        let file3 = temp_dir.path().join("memory").join("systems.md");
        tokio::fs::write(&file3, "Systems programming low level memory allocation garbage collection runtime performance").await.unwrap();

        mm.load_document(&file1).await.unwrap();
        mm.load_document(&file2).await.unwrap();
        mm.load_document(&file3).await.unwrap();
        mm.rebuild_index().await.unwrap();

        // Search for programming-related content
        let query = MemoryQuery {
            query: "programming memory safety".to_string(),
            limit: Some(3),
            filters: HashMap::new(),
            semantic: true,
        };

        let results = mm.search(query).await.unwrap();
        assert!(results.total_results >= 2, "Should find at least 2 relevant chunks");

        // The rust document should be the top result
        assert!(
            results.chunks[0].chunk.document_id == "rust.md",
            "Rust doc should be most relevant for 'programming memory safety', got: {}",
            results.chunks[0].chunk.document_id
        );
    }

    #[tokio::test]
    async fn test_semantic_search_ranking() {
        let temp_dir = TempDir::new().unwrap();
        let mm = MemoryManager::new(temp_dir.path().to_path_buf()).await.unwrap();

        let file1 = temp_dir.path().join("memory").join("agents.md");
        tokio::fs::write(&file1, "AI agent system tool execution loop detection circuit breaker").await.unwrap();

        let file2 = temp_dir.path().join("memory").join("credentials.md");
        tokio::fs::write(&file2, "Credential vault encryption AES Argon2 password keyfile permissions").await.unwrap();

        mm.load_document(&file1).await.unwrap();
        mm.load_document(&file2).await.unwrap();
        mm.rebuild_index().await.unwrap();

        let query = MemoryQuery {
            query: "agent tool execution".to_string(),
            limit: Some(5),
            filters: HashMap::new(),
            semantic: true,
        };

        let results = mm.search(query).await.unwrap();
        assert!(!results.chunks.is_empty());
        assert_eq!(results.chunks[0].chunk.document_id, "agents.md");
    }

    #[tokio::test]
    async fn test_semantic_search_no_results() {
        let temp_dir = TempDir::new().unwrap();
        let mm = MemoryManager::new(temp_dir.path().to_path_buf()).await.unwrap();

        // No documents loaded
        let query = MemoryQuery {
            query: "anything".to_string(),
            limit: Some(5),
            filters: HashMap::new(),
            semantic: true,
        };

        let results = mm.search(query).await.unwrap();
        assert_eq!(results.total_results, 0);
    }

    #[tokio::test]
    async fn test_cosine_similarity() {
        let mut doc_freq = HashMap::new();
        doc_freq.insert("hello".to_string(), 1);
        doc_freq.insert("world".to_string(), 2);
        doc_freq.insert("foo".to_string(), 1);

        let mut tf1 = HashMap::new();
        tf1.insert("hello".to_string(), 2);
        tf1.insert("world".to_string(), 1);

        let mut tf2 = HashMap::new();
        tf2.insert("hello".to_string(), 1);
        tf2.insert("world".to_string(), 1);

        let v1 = SparseVector::from_term_freqs(&tf1, &doc_freq, 3);
        let v2 = SparseVector::from_term_freqs(&tf2, &doc_freq, 3);

        let sim = v1.cosine_similarity(&v2);
        assert!(sim > 0.9, "Similar vectors should have high cosine similarity: {sim}");

        // Completely disjoint vectors
        let mut tf3 = HashMap::new();
        tf3.insert("foo".to_string(), 1);
        let v3 = SparseVector::from_term_freqs(&tf3, &doc_freq, 3);

        let sim2 = v1.cosine_similarity(&v3);
        assert!(sim2 < 0.01, "Disjoint vectors should have near-zero similarity: {sim2}");
    }

    #[tokio::test]
    async fn test_tokenize() {
        let tokens = tokenize("Hello, World! This is a test-document with code_names.");
        assert!(tokens.contains(&"hello".to_string()));
        assert!(tokens.contains(&"world".to_string()));
        assert!(tokens.contains(&"test".to_string()));
        assert!(tokens.contains(&"document".to_string()));
        assert!(tokens.contains(&"code_names".to_string()));
        // Stop words should be excluded
        assert!(!tokens.contains(&"is".to_string()));
        assert!(!tokens.contains(&"a".to_string()));
    }

    #[tokio::test]
    async fn test_overlapping_chunks() {
        let temp_dir = TempDir::new().unwrap();
        let mm = MemoryManager::new(temp_dir.path().to_path_buf()).await.unwrap();

        // Create a document with 25 lines
        let content: String = (1..=25).map(|i| format!("Line {i} of the document")).collect::<Vec<_>>().join("\n");
        let file = temp_dir.path().join("memory").join("long.md");
        tokio::fs::write(&file, &content).await.unwrap();

        mm.load_document(&file).await.unwrap();

        let docs = mm.documents.read().await;
        let doc = docs.get("long.md").unwrap();
        // With chunk_size=10, overlap=3, step=7: chunks start at 0, 7, 14, 21
        assert!(doc.chunks.len() >= 4, "Should have at least 4 overlapping chunks, got {}", doc.chunks.len());
        // Verify overlap: chunk 0 ends at 10, chunk 1 starts at 7
        assert_eq!(doc.chunks[0].start_offset, 0);
        assert_eq!(doc.chunks[1].start_offset, 7);
    }

    /// Mock embedding provider that returns simple hash-based vectors
    struct MockEmbeddingProvider;

    #[async_trait::async_trait]
    impl EmbeddingProvider for MockEmbeddingProvider {
        async fn embed(&self, text: &str) -> Result<Vec<f32>> {
            // Simple deterministic embedding: 8-dimensional vector based on character distribution
            let mut vec = vec![0.0_f32; 8];
            for (i, ch) in text.chars().enumerate() {
                let idx = (ch as usize + i) % 8;
                vec[idx] += 1.0;
            }
            // Normalize
            let norm: f32 = vec.iter().map(|v| v * v).sum::<f32>().sqrt();
            if norm > 0.0 {
                for v in &mut vec {
                    *v /= norm;
                }
            }
            Ok(vec)
        }
    }

    #[tokio::test]
    async fn test_hybrid_search_with_embeddings() {
        let temp_dir = TempDir::new().unwrap();
        let mut mm = MemoryManager::new(temp_dir.path().to_path_buf()).await.unwrap();
        mm.set_embedding_provider(Arc::new(MockEmbeddingProvider));

        let file1 = temp_dir.path().join("memory").join("rust.md");
        tokio::fs::write(&file1, "Rust programming language memory safety ownership borrowing compiler").await.unwrap();

        let file2 = temp_dir.path().join("memory").join("cooking.md");
        tokio::fs::write(&file2, "Recipe for chocolate cake flour sugar eggs butter baking oven temperature").await.unwrap();

        mm.load_document(&file1).await.unwrap();
        mm.load_document(&file2).await.unwrap();
        mm.rebuild_index().await.unwrap();

        // Verify embeddings were computed
        {
            let idx = mm.vector_index.read().await;
            assert!(!idx.embeddings.is_empty(), "Embeddings should have been computed");
        }

        let query = MemoryQuery {
            query: "programming".to_string(),
            limit: Some(3),
            filters: HashMap::new(),
            semantic: true,
        };

        let results = mm.search(query).await.unwrap();
        assert!(results.total_results > 0);
    }

    #[tokio::test]
    async fn test_fallback_to_tfidf_without_embeddings() {
        let temp_dir = TempDir::new().unwrap();
        let mm = MemoryManager::new(temp_dir.path().to_path_buf()).await.unwrap();
        // No embedding provider set

        let file = temp_dir.path().join("memory").join("test.md");
        tokio::fs::write(&file, "Some searchable content about programming").await.unwrap();
        mm.load_document(&file).await.unwrap();
        mm.rebuild_index().await.unwrap();

        // Embeddings should be empty
        {
            let idx = mm.vector_index.read().await;
            assert!(idx.embeddings.is_empty());
        }

        let query = MemoryQuery {
            query: "programming".to_string(),
            limit: Some(3),
            filters: HashMap::new(),
            semantic: true,
        };

        // Should still work via TF-IDF only
        let results = mm.search(query).await.unwrap();
        assert!(results.total_results > 0);
    }

    #[test]
    fn test_dense_vector_cosine_similarity() {
        let v1 = DenseVector::new(vec![1.0, 0.0, 0.0]);
        let v2 = DenseVector::new(vec![1.0, 0.0, 0.0]);
        assert!((v1.cosine_similarity(&v2) - 1.0).abs() < 0.001);

        let v3 = DenseVector::new(vec![0.0, 1.0, 0.0]);
        assert!(v1.cosine_similarity(&v3).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_index_rebuild_after_load() {
        let temp_dir = TempDir::new().unwrap();
        let mm = MemoryManager::new(temp_dir.path().to_path_buf()).await.unwrap();

        let file = temp_dir.path().join("memory").join("doc.md");
        tokio::fs::write(&file, "Some document content here").await.unwrap();
        mm.load_document(&file).await.unwrap();

        // Index should be dirty
        {
            let idx = mm.vector_index.read().await;
            assert!(idx.dirty);
        }

        // Search triggers rebuild
        let query = MemoryQuery {
            query: "document".to_string(),
            limit: Some(5),
            filters: HashMap::new(),
            semantic: true,
        };
        let results = mm.search(query).await.unwrap();
        assert!(results.total_results > 0);

        // Index should no longer be dirty
        let idx = mm.vector_index.read().await;
        assert!(!idx.dirty);
        assert!(idx.total_chunks > 0);
    }

    #[tokio::test]
    async fn test_episodic_store_and_recall() {
        let temp_dir = TempDir::new().unwrap();
        let store = EpisodicStore::new(temp_dir.path().to_path_buf());

        let ep1 = Episode {
            session_id: "s1".to_string(),
            timestamp: Utc::now(),
            summary: "User asked to refactor the database module".to_string(),
            outcome: "success".to_string(),
            tools_used: vec!["read".to_string(), "edit".to_string()],
            tokens_used: 5000,
        };
        let ep2 = Episode {
            session_id: "s2".to_string(),
            timestamp: Utc::now(),
            summary: "User asked to fix a bug in the web server".to_string(),
            outcome: "success".to_string(),
            tools_used: vec!["grep".to_string(), "edit".to_string()],
            tokens_used: 3000,
        };

        store.store("agent-1", &ep1).await.unwrap();
        store.store("agent-1", &ep2).await.unwrap();

        assert_eq!(store.episode_count("agent-1").await.unwrap(), 2);

        let results = store.recall("agent-1", "database refactor", 5).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].session_id, "s1");

        let results = store.recall("agent-1", "web server bug", 5).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].session_id, "s2");
    }

    #[tokio::test]
    async fn test_episodic_store_empty_recall() {
        let temp_dir = TempDir::new().unwrap();
        let store = EpisodicStore::new(temp_dir.path().to_path_buf());

        let results = store.recall("nonexistent", "anything", 5).await.unwrap();
        assert!(results.is_empty());

        assert_eq!(store.episode_count("nonexistent").await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_episodic_store_limit() {
        let temp_dir = TempDir::new().unwrap();
        let store = EpisodicStore::new(temp_dir.path().to_path_buf());

        for i in 0..10 {
            let ep = Episode {
                session_id: format!("s{i}"),
                timestamp: Utc::now(),
                summary: format!("Episode {i} about testing code quality"),
                outcome: "success".to_string(),
                tools_used: vec![],
                tokens_used: 100,
            };
            store.store("agent-1", &ep).await.unwrap();
        }

        let results = store.recall("agent-1", "testing code", 3).await.unwrap();
        assert_eq!(results.len(), 3);
    }
}
