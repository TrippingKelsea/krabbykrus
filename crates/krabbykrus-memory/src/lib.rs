//! Memory management system for Krabbykrus

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
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

/// Memory manager handles file-based memory storage and vector search
pub struct MemoryManager {
    workspace_path: PathBuf,
    documents: tokio::sync::RwLock<HashMap<String, MemoryDocument>>,
    vector_index: tokio::sync::RwLock<VectorIndex>,
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

/// Vector index for semantic search
#[derive(Debug)]
struct VectorIndex {
    embeddings: HashMap<String, Vec<f32>>,
    chunks: HashMap<String, MemoryChunk>,
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
                embeddings: HashMap::new(),
                chunks: HashMap::new(),
            }),
        })
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
            let daily_log = memory_dir.join(format!("{}.md", date));
            if daily_log.exists() {
                self.load_document(&daily_log).await?;
            }
        }
        
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
        
        Ok(())
    }
    
    /// Create chunks from document content
    fn create_chunks(&self, document_id: &str, content: &str) -> Vec<MemoryChunk> {
        let mut chunks = Vec::new();
        let lines: Vec<&str> = content.lines().collect();
        let chunk_size = 10; // Lines per chunk
        
        for (i, chunk_lines) in lines.chunks(chunk_size).enumerate() {
            let chunk_content = chunk_lines.join("\n");
            let start_line = i * chunk_size;
            let end_line = start_line + chunk_lines.len();
            
            let chunk = MemoryChunk {
                id: format!("{}:chunk:{}", document_id, i),
                document_id: document_id.to_string(),
                content: chunk_content,
                start_offset: start_line,
                end_offset: end_line,
                metadata: HashMap::new(),
            };
            
            chunks.push(chunk);
        }
        
        chunks
    }
    
    /// Search memory
    pub async fn search(&self, query: MemoryQuery) -> Result<MemoryResult> {
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
    
    /// Perform semantic search (placeholder implementation)
    async fn semantic_search(&self, query: MemoryQuery) -> Result<MemoryResult> {
        // For now, fallback to keyword search
        // In a full implementation, this would use actual vector embeddings
        tracing::warn!("Semantic search not fully implemented, falling back to keyword search");
        self.keyword_search(query).await
    }
    
    /// Create a highlight snippet
    fn create_highlight(&self, content: &str, query: &str) -> String {
        let query_lower = query.to_lowercase();
        let content_lower = content.to_lowercase();
        
        if let Some(pos) = content_lower.find(&query_lower) {
            let start = pos.saturating_sub(50);
            let end = (pos + query.len() + 50).min(content.len());
            let snippet = &content[start..end];
            
            // Replace query with highlighted version
            snippet.replace(query, &format!("**{}**", query))
        } else {
            content.chars().take(100).collect()
        }
    }
    
    /// Write to daily log
    pub async fn write_daily_log(&self, content: &str) -> Result<()> {
        let date = Utc::now().format("%Y-%m-%d").to_string();
        let log_path = self.workspace_path.join("memory").join(format!("{}.md", date));
        
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
}

/// Default vector index implementation
impl Default for VectorIndex {
    fn default() -> Self {
        Self {
            embeddings: HashMap::new(),
            chunks: HashMap::new(),
        }
    }
}

/// Mock memory manager for testing
pub struct MockMemoryManager;

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
    use super::*;
    use tempfile::TempDir;
    
    #[tokio::test]
    async fn test_memory_manager_creation() {
        let temp_dir = TempDir::new().unwrap();
        let memory_manager = MemoryManager::new(temp_dir.path().to_path_buf()).await.unwrap();
        
        // Should create memory directory
        assert!(temp_dir.path().join("memory").exists());
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
}