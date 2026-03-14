//! Codebase indexing and repo map generation.
//!
//! Provides symbol extraction per language, TF-IDF relevance scoring, and
//! a compact "repo map" format that gives agents an overview of the codebase
//! structure without reading every file.

use regex::Regex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::debug;

/// A symbol extracted from a source file.
#[derive(Debug, Clone)]
pub struct Symbol {
    /// Symbol name (function, class, struct, etc.)
    pub name: String,
    /// Symbol kind
    pub kind: SymbolKind,
    /// Line number (1-indexed)
    pub line: usize,
    /// Parent symbol (e.g., method inside a class)
    pub parent: Option<String>,
}

/// Kind of extracted symbol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    Class,
    Struct,
    Enum,
    Interface,
    Trait,
    Method,
    Constant,
    Module,
    Type,
}

impl std::fmt::Display for SymbolKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Function => write!(f, "fn"),
            Self::Class => write!(f, "class"),
            Self::Struct => write!(f, "struct"),
            Self::Enum => write!(f, "enum"),
            Self::Interface => write!(f, "interface"),
            Self::Trait => write!(f, "trait"),
            Self::Method => write!(f, "method"),
            Self::Constant => write!(f, "const"),
            Self::Module => write!(f, "mod"),
            Self::Type => write!(f, "type"),
        }
    }
}

/// Indexed representation of a source file.
#[derive(Debug, Clone)]
pub struct IndexedFile {
    /// Relative path from workspace root.
    pub path: PathBuf,
    /// Detected language.
    pub language: Language,
    /// Extracted symbols.
    pub symbols: Vec<Symbol>,
    /// File size in bytes.
    pub size: u64,
}

/// Detected programming language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Go,
    Java,
    CSharp,
    Ruby,
    Unknown,
}

impl Language {
    /// Detect language from file extension.
    pub fn from_extension(ext: &str) -> Self {
        match ext {
            "rs" => Self::Rust,
            "py" => Self::Python,
            "js" | "jsx" | "mjs" | "cjs" => Self::JavaScript,
            "ts" | "tsx" | "mts" => Self::TypeScript,
            "go" => Self::Go,
            "java" => Self::Java,
            "cs" => Self::CSharp,
            "rb" => Self::Ruby,
            _ => Self::Unknown,
        }
    }
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rust => write!(f, "Rust"),
            Self::Python => write!(f, "Python"),
            Self::JavaScript => write!(f, "JavaScript"),
            Self::TypeScript => write!(f, "TypeScript"),
            Self::Go => write!(f, "Go"),
            Self::Java => write!(f, "Java"),
            Self::CSharp => write!(f, "C#"),
            Self::Ruby => write!(f, "Ruby"),
            Self::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Extract symbols from source code based on language.
pub fn extract_symbols(content: &str, language: Language) -> Vec<Symbol> {
    match language {
        Language::Rust => extract_rust_symbols(content),
        Language::Python => extract_python_symbols(content),
        Language::JavaScript | Language::TypeScript => extract_js_ts_symbols(content),
        Language::Go => extract_go_symbols(content),
        Language::Java | Language::CSharp => extract_java_like_symbols(content),
        Language::Ruby => extract_ruby_symbols(content),
        Language::Unknown => vec![],
    }
}

fn extract_rust_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    let fn_re = Regex::new(r"^\s*(?:pub\s+)?(?:async\s+)?fn\s+(\w+)").expect("valid regex");
    let struct_re = Regex::new(r"^\s*(?:pub\s+)?struct\s+(\w+)").expect("valid regex");
    let enum_re = Regex::new(r"^\s*(?:pub\s+)?enum\s+(\w+)").expect("valid regex");
    let trait_re = Regex::new(r"^\s*(?:pub\s+)?trait\s+(\w+)").expect("valid regex");
    let impl_re = Regex::new(r"^\s*impl(?:<[^>]*>)?\s+(?:(\w+)\s+for\s+)?(\w+)").expect("valid regex");
    let mod_re = Regex::new(r"^\s*(?:pub\s+)?mod\s+(\w+)").expect("valid regex");
    let type_re = Regex::new(r"^\s*(?:pub\s+)?type\s+(\w+)").expect("valid regex");
    let const_re = Regex::new(r"^\s*(?:pub\s+)?const\s+(\w+)").expect("valid regex");

    let mut current_impl: Option<String> = None;

    for (i, line) in content.lines().enumerate() {
        let line_num = i + 1;

        if let Some(cap) = impl_re.captures(line) {
            current_impl = cap.get(2).map(|m| m.as_str().to_string());
        }

        if let Some(cap) = fn_re.captures(line) {
            let name = cap[1].to_string();
            symbols.push(Symbol {
                name,
                kind: if current_impl.is_some() { SymbolKind::Method } else { SymbolKind::Function },
                line: line_num,
                parent: current_impl.clone(),
            });
        } else if let Some(cap) = struct_re.captures(line) {
            symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Struct, line: line_num, parent: None });
            current_impl = None;
        } else if let Some(cap) = enum_re.captures(line) {
            symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Enum, line: line_num, parent: None });
            current_impl = None;
        } else if let Some(cap) = trait_re.captures(line) {
            symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Trait, line: line_num, parent: None });
            current_impl = None;
        } else if let Some(cap) = mod_re.captures(line) {
            symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Module, line: line_num, parent: None });
        } else if let Some(cap) = type_re.captures(line) {
            symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Type, line: line_num, parent: None });
        } else if let Some(cap) = const_re.captures(line) {
            symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Constant, line: line_num, parent: None });
        }

        // Reset impl context when we see a closing brace at column 0
        if line.starts_with('}') {
            current_impl = None;
        }
    }

    symbols
}

fn extract_python_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    let class_re = Regex::new(r"^class\s+(\w+)").expect("valid regex");
    let def_re = Regex::new(r"^(\s*)def\s+(\w+)").expect("valid regex");

    let mut current_class: Option<String> = None;

    for (i, line) in content.lines().enumerate() {
        let line_num = i + 1;
        if let Some(cap) = class_re.captures(line) {
            let name = cap[1].to_string();
            current_class = Some(name.clone());
            symbols.push(Symbol { name, kind: SymbolKind::Class, line: line_num, parent: None });
        } else if let Some(cap) = def_re.captures(line) {
            let indent = cap[1].len();
            let name = cap[2].to_string();
            if indent > 0 && current_class.is_some() {
                symbols.push(Symbol { name, kind: SymbolKind::Method, line: line_num, parent: current_class.clone() });
            } else {
                current_class = None;
                symbols.push(Symbol { name, kind: SymbolKind::Function, line: line_num, parent: None });
            }
        }
    }

    symbols
}

fn extract_js_ts_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    let fn_re = Regex::new(r"^(?:export\s+)?(?:async\s+)?function\s+(\w+)").expect("valid regex");
    let class_re = Regex::new(r"^(?:export\s+)?class\s+(\w+)").expect("valid regex");
    let interface_re = Regex::new(r"^(?:export\s+)?interface\s+(\w+)").expect("valid regex");
    let const_re = Regex::new(r"^(?:export\s+)?const\s+(\w+)\s*=").expect("valid regex");
    let type_re = Regex::new(r"^(?:export\s+)?type\s+(\w+)\s*=").expect("valid regex");

    for (i, line) in content.lines().enumerate() {
        let line_num = i + 1;
        let trimmed = line.trim_start();
        if let Some(cap) = fn_re.captures(trimmed) {
            symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Function, line: line_num, parent: None });
        } else if let Some(cap) = class_re.captures(trimmed) {
            symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Class, line: line_num, parent: None });
        } else if let Some(cap) = interface_re.captures(trimmed) {
            symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Interface, line: line_num, parent: None });
        } else if let Some(cap) = type_re.captures(trimmed) {
            symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Type, line: line_num, parent: None });
        } else if let Some(cap) = const_re.captures(trimmed) {
            symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Constant, line: line_num, parent: None });
        }
    }

    symbols
}

fn extract_go_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    let fn_re = Regex::new(r"^func\s+(?:\([^)]+\)\s+)?(\w+)").expect("valid regex");
    let type_re = Regex::new(r"^type\s+(\w+)\s+(struct|interface)").expect("valid regex");

    for (i, line) in content.lines().enumerate() {
        let line_num = i + 1;
        if let Some(cap) = fn_re.captures(line) {
            symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Function, line: line_num, parent: None });
        } else if let Some(cap) = type_re.captures(line) {
            let kind = if &cap[2] == "struct" { SymbolKind::Struct } else { SymbolKind::Interface };
            symbols.push(Symbol { name: cap[1].to_string(), kind, line: line_num, parent: None });
        }
    }

    symbols
}

fn extract_java_like_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    let class_re = Regex::new(r"(?:public|private|protected)?\s*(?:static\s+)?(?:abstract\s+)?class\s+(\w+)").expect("valid regex");
    let interface_re = Regex::new(r"(?:public\s+)?interface\s+(\w+)").expect("valid regex");
    let method_re = Regex::new(r"^\s+(?:public|private|protected)\s+(?:static\s+)?(?:\w+(?:<[^>]+>)?)\s+(\w+)\s*\(").expect("valid regex");

    for (i, line) in content.lines().enumerate() {
        let line_num = i + 1;
        if let Some(cap) = class_re.captures(line) {
            symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Class, line: line_num, parent: None });
        } else if let Some(cap) = interface_re.captures(line) {
            symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Interface, line: line_num, parent: None });
        } else if let Some(cap) = method_re.captures(line) {
            symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Method, line: line_num, parent: None });
        }
    }

    symbols
}

fn extract_ruby_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    let class_re = Regex::new(r"^class\s+(\w+)").expect("valid regex");
    let module_re = Regex::new(r"^module\s+(\w+)").expect("valid regex");
    let def_re = Regex::new(r"^\s*def\s+(?:self\.)?(\w+)").expect("valid regex");

    for (i, line) in content.lines().enumerate() {
        let line_num = i + 1;
        if let Some(cap) = class_re.captures(line) {
            symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Class, line: line_num, parent: None });
        } else if let Some(cap) = module_re.captures(line) {
            symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Module, line: line_num, parent: None });
        } else if let Some(cap) = def_re.captures(line) {
            symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Method, line: line_num, parent: None });
        }
    }

    symbols
}

/// Default extensions to index.
const INDEXABLE_EXTENSIONS: &[&str] = &[
    "rs", "py", "js", "jsx", "ts", "tsx", "go", "java", "cs", "rb",
    "mjs", "cjs", "mts",
];

/// Default directories to skip.
const SKIP_DIRS: &[&str] = &[
    "target", "node_modules", ".git", "vendor", "dist", "build",
    "__pycache__", ".venv", "venv", ".next", ".nuxt",
];

/// Index a workspace directory and return all indexed files.
pub async fn index_workspace(root: &Path) -> Vec<IndexedFile> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(e) => e,
            Err(_) => continue,
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            let fname = entry.file_name();
            let fname_str = fname.to_str().unwrap_or("");

            if path.is_dir() {
                if !SKIP_DIRS.contains(&fname_str) && !fname_str.starts_with('.') {
                    stack.push(path);
                }
                continue;
            }

            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !INDEXABLE_EXTENSIONS.contains(&ext) {
                continue;
            }

            let language = Language::from_extension(ext);
            let metadata = match tokio::fs::metadata(&path).await {
                Ok(m) => m,
                Err(_) => continue,
            };

            // Skip very large files (> 1MB)
            if metadata.len() > 1_000_000 {
                continue;
            }

            let content = match tokio::fs::read_to_string(&path).await {
                Ok(c) => c,
                Err(_) => continue,
            };

            let symbols = extract_symbols(&content, language);
            let rel_path = path.strip_prefix(root).unwrap_or(&path).to_path_buf();

            debug!("Indexed {} ({} symbols)", rel_path.display(), symbols.len());

            files.push(IndexedFile {
                path: rel_path,
                language,
                symbols,
                size: metadata.len(),
            });
        }
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

/// Generate a compact repo map string from indexed files.
///
/// The repo map shows the file tree with key symbols, suitable for
/// injection into agent system prompts.
pub fn generate_repo_map(files: &[IndexedFile], max_chars: usize) -> String {
    let mut output = String::from("# Repository Map\n\n");
    let mut remaining = max_chars.saturating_sub(output.len());

    for file in files {
        if remaining < 50 {
            output.push_str("\n... (truncated)\n");
            break;
        }

        let mut file_section = format!("## {}\n", file.path.display());

        for sym in &file.symbols {
            let line = match &sym.parent {
                Some(parent) => format!("  {} {}::{} (L{})\n", sym.kind, parent, sym.name, sym.line),
                None => format!("  {} {} (L{})\n", sym.kind, sym.name, sym.line),
            };
            file_section.push_str(&line);
        }

        file_section.push('\n');

        if file_section.len() > remaining {
            break;
        }

        remaining -= file_section.len();
        output.push_str(&file_section);
    }

    output
}

/// Compute TF-IDF relevance scores for files relative to a query.
///
/// Returns files sorted by relevance score (highest first).
pub fn rank_files_by_relevance(
    files: &[IndexedFile],
    query: &str,
) -> Vec<(usize, f64)> {
    let query_terms: Vec<&str> = query.split_whitespace()
        .map(|t| t.trim_matches(|c: char| !c.is_alphanumeric() && c != '_'))
        .filter(|t| t.len() > 1)
        .collect();

    if query_terms.is_empty() {
        return Vec::new();
    }

    let n_docs = files.len() as f64;
    let mut scores: Vec<(usize, f64)> = Vec::new();

    // Compute IDF for each query term
    let mut idf: HashMap<&str, f64> = HashMap::new();
    for term in &query_terms {
        let doc_freq = files.iter().filter(|f| {
            f.symbols.iter().any(|s| s.name.to_lowercase().contains(&term.to_lowercase()))
                || f.path.to_str().is_some_and(|p| p.to_lowercase().contains(&term.to_lowercase()))
        }).count() as f64;

        let idf_val = if doc_freq > 0.0 {
            (n_docs / doc_freq).ln() + 1.0
        } else {
            0.0
        };
        idf.insert(term, idf_val);
    }

    for (idx, file) in files.iter().enumerate() {
        let mut score = 0.0;

        for term in &query_terms {
            let term_lower = term.to_lowercase();
            let idf_val = idf.get(term).copied().unwrap_or(0.0);

            // Term frequency in symbol names
            let sym_tf = file.symbols.iter()
                .filter(|s| s.name.to_lowercase().contains(&term_lower))
                .count() as f64;

            // Term frequency in path
            let path_tf = if file.path.to_str().is_some_and(|p| p.to_lowercase().contains(&term_lower)) {
                2.0 // Boost for path matches
            } else {
                0.0
            };

            score += (sym_tf + path_tf) * idf_val;
        }

        if score > 0.0 {
            scores.push((idx, score));
        }
    }

    scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scores
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn test_extract_rust_symbols() {
        let code = r#"
pub struct Config {
    pub field: String,
}

pub enum Color {
    Red,
    Blue,
}

pub trait Drawable {
    fn draw(&self);
}

impl Config {
    pub fn new() -> Self { Config { field: String::new() } }
    pub async fn load(path: &str) -> Self { todo!() }
}

fn helper() {}

pub mod utils;
pub type Result<T> = std::result::Result<T, Error>;
pub const MAX_SIZE: usize = 1024;
"#;
        let symbols = extract_rust_symbols(code);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Config"));
        assert!(names.contains(&"Color"));
        assert!(names.contains(&"Drawable"));
        assert!(names.contains(&"new"));
        assert!(names.contains(&"load"));
        assert!(names.contains(&"helper"));
        assert!(names.contains(&"utils"));
        assert!(names.contains(&"Result"));
        assert!(names.contains(&"MAX_SIZE"));

        // Check that "new" is a Method with parent "Config"
        let new_sym = symbols.iter().find(|s| s.name == "new").unwrap();
        assert_eq!(new_sym.kind, SymbolKind::Method);
        assert_eq!(new_sym.parent.as_deref(), Some("Config"));
    }

    #[test]
    fn test_extract_python_symbols() {
        let code = r#"
class MyClass:
    def __init__(self):
        pass

    def method(self):
        pass

def standalone():
    pass
"#;
        let symbols = extract_python_symbols(code);
        assert!(symbols.iter().any(|s| s.name == "MyClass" && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.name == "__init__" && s.kind == SymbolKind::Method));
        assert!(symbols.iter().any(|s| s.name == "standalone" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_extract_js_ts_symbols() {
        let code = r#"
export function handleRequest(req) {}
export class Router {}
export interface Config {}
export type Handler = () => void;
export const MAX_RETRIES = 3;
"#;
        let symbols = extract_js_ts_symbols(code);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"handleRequest"));
        assert!(names.contains(&"Router"));
        assert!(names.contains(&"Config"));
        assert!(names.contains(&"Handler"));
        assert!(names.contains(&"MAX_RETRIES"));
    }

    #[test]
    fn test_extract_go_symbols() {
        let code = r#"
func main() {}
func (s *Server) Start() error {}
type Config struct {}
type Handler interface {}
"#;
        let symbols = extract_go_symbols(code);
        assert!(symbols.iter().any(|s| s.name == "main" && s.kind == SymbolKind::Function));
        assert!(symbols.iter().any(|s| s.name == "Start" && s.kind == SymbolKind::Function));
        assert!(symbols.iter().any(|s| s.name == "Config" && s.kind == SymbolKind::Struct));
        assert!(symbols.iter().any(|s| s.name == "Handler" && s.kind == SymbolKind::Interface));
    }

    #[test]
    fn test_language_detection() {
        assert_eq!(Language::from_extension("rs"), Language::Rust);
        assert_eq!(Language::from_extension("py"), Language::Python);
        assert_eq!(Language::from_extension("tsx"), Language::TypeScript);
        assert_eq!(Language::from_extension("go"), Language::Go);
        assert_eq!(Language::from_extension("java"), Language::Java);
        assert_eq!(Language::from_extension("txt"), Language::Unknown);
    }

    #[test]
    fn test_generate_repo_map() {
        let files = vec![
            IndexedFile {
                path: PathBuf::from("src/main.rs"),
                language: Language::Rust,
                symbols: vec![
                    Symbol { name: "main".to_string(), kind: SymbolKind::Function, line: 1, parent: None },
                    Symbol { name: "Config".to_string(), kind: SymbolKind::Struct, line: 10, parent: None },
                ],
                size: 500,
            },
            IndexedFile {
                path: PathBuf::from("src/lib.rs"),
                language: Language::Rust,
                symbols: vec![
                    Symbol { name: "process".to_string(), kind: SymbolKind::Function, line: 5, parent: None },
                ],
                size: 300,
            },
        ];

        let map = generate_repo_map(&files, 1000);
        assert!(map.contains("src/main.rs"));
        assert!(map.contains("fn main"));
        assert!(map.contains("struct Config"));
        assert!(map.contains("src/lib.rs"));
        assert!(map.contains("fn process"));
    }

    #[test]
    fn test_generate_repo_map_truncation() {
        let files = vec![
            IndexedFile {
                path: PathBuf::from("src/very_long_file.rs"),
                language: Language::Rust,
                symbols: (0..100).map(|i| Symbol {
                    name: format!("function_{i}"),
                    kind: SymbolKind::Function,
                    line: i + 1,
                    parent: None,
                }).collect(),
                size: 5000,
            },
        ];

        let map = generate_repo_map(&files, 200);
        assert!(map.len() <= 300); // Some flexibility for header
    }

    #[test]
    fn test_rank_files_by_relevance() {
        let files = vec![
            IndexedFile {
                path: PathBuf::from("src/config.rs"),
                language: Language::Rust,
                symbols: vec![
                    Symbol { name: "Config".to_string(), kind: SymbolKind::Struct, line: 1, parent: None },
                    Symbol { name: "load_config".to_string(), kind: SymbolKind::Function, line: 10, parent: None },
                ],
                size: 500,
            },
            IndexedFile {
                path: PathBuf::from("src/server.rs"),
                language: Language::Rust,
                symbols: vec![
                    Symbol { name: "Server".to_string(), kind: SymbolKind::Struct, line: 1, parent: None },
                    Symbol { name: "start".to_string(), kind: SymbolKind::Method, line: 20, parent: Some("Server".to_string()) },
                ],
                size: 800,
            },
        ];

        let ranked = rank_files_by_relevance(&files, "config load");
        assert!(!ranked.is_empty());
        // config.rs should rank higher for "config load"
        assert_eq!(ranked[0].0, 0);
    }

    #[test]
    fn test_rank_empty_query() {
        let files = vec![
            IndexedFile {
                path: PathBuf::from("src/main.rs"),
                language: Language::Rust,
                symbols: vec![],
                size: 100,
            },
        ];
        let ranked = rank_files_by_relevance(&files, "");
        assert!(ranked.is_empty());
    }

    #[tokio::test]
    async fn test_index_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let src_dir = dir.path().join("src");
        tokio::fs::create_dir_all(&src_dir).await.unwrap();

        tokio::fs::write(
            src_dir.join("main.rs"),
            "fn main() {}\nstruct App;\n",
        ).await.unwrap();

        tokio::fs::write(
            src_dir.join("lib.py"),
            "class Helper:\n    def run(self):\n        pass\n",
        ).await.unwrap();

        // Non-indexable file
        tokio::fs::write(
            src_dir.join("readme.txt"),
            "This should be skipped",
        ).await.unwrap();

        let files = index_workspace(dir.path()).await;
        assert_eq!(files.len(), 2); // .rs and .py, not .txt

        let rs_file = files.iter().find(|f| f.path.to_str().unwrap().ends_with(".rs")).unwrap();
        assert_eq!(rs_file.language, Language::Rust);
        assert!(rs_file.symbols.iter().any(|s| s.name == "main"));
        assert!(rs_file.symbols.iter().any(|s| s.name == "App"));

        let py_file = files.iter().find(|f| f.path.to_str().unwrap().ends_with(".py")).unwrap();
        assert_eq!(py_file.language, Language::Python);
        assert!(py_file.symbols.iter().any(|s| s.name == "Helper"));
    }

    #[tokio::test]
    async fn test_index_workspace_skips_hidden_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let hidden = dir.path().join(".hidden");
        tokio::fs::create_dir_all(&hidden).await.unwrap();
        tokio::fs::write(hidden.join("secret.rs"), "fn secret() {}").await.unwrap();

        let files = index_workspace(dir.path()).await;
        assert!(files.is_empty());
    }
}
