//! Version-aware config migration rules.
//!
//! Static table of known renames is checked first (high confidence).
//! The AI model provides a fallback for unknowns (lower confidence).

/// A known field rename across versions.
struct KnownRename {
    old_path: &'static str,
    new_path: &'static str,
    since_version: &'static str,
}

/// Static migration table. Add entries here as the config schema evolves.
///
/// These are checked deterministically before invoking the AI model.
const MIGRATION_TABLE: &[KnownRename] = &[
    KnownRename {
        old_path: "agents.list",
        new_path: "vault:agents",
        since_version: "0.3.0",
    },
];

/// A migration note — either from the static table or AI detection.
#[derive(Debug, Clone)]
pub struct MigrationNote {
    /// The old/deprecated field path.
    pub old_path: String,
    /// The new field path (None = removed, not renamed).
    pub new_path: Option<String>,
    /// The version since which this field was deprecated.
    pub since_version: Option<String>,
    /// How this migration was detected.
    pub source: MigrationSource,
}

/// How a migration note was detected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationSource {
    /// Found in the static migration table — high confidence.
    StaticTable,
    /// Found by the AI model — show with lower confidence marker.
    AiDetected,
    /// Recalled from the learned fix store.
    Learned,
}

/// Check the raw TOML against the static migration table.
pub fn check_static_table(raw_toml: &str) -> Vec<MigrationNote> {
    // Parse the TOML permissively to check field presence
    let value: toml::Value = match raw_toml.parse() {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let mut notes = Vec::new();
    for rename in MIGRATION_TABLE {
        if field_exists(&value, rename.old_path) {
            notes.push(MigrationNote {
                old_path: rename.old_path.to_string(),
                new_path: Some(rename.new_path.to_string()),
                since_version: Some(rename.since_version.to_string()),
                source: MigrationSource::StaticTable,
            });
        }
    }
    notes
}

/// Format known renames as a string for the AI prompt.
pub fn format_known_renames() -> String {
    if MIGRATION_TABLE.is_empty() {
        return "(no known renames yet)".to_string();
    }
    MIGRATION_TABLE
        .iter()
        .map(|r| {
            format!(
                "{} -> {} (since {})",
                r.old_path, r.new_path, r.since_version
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parse the AI model's migration output.
///
/// Expected format:
/// - `DEPRECATED: old.path -> new.path`
/// - `NONE`
pub fn parse_migration_output(output: &str) -> Vec<MigrationNote> {
    let mut notes = Vec::new();

    for line in output.lines() {
        let line = line.trim();
        if line == "NONE" {
            break;
        }
        if let Some(rest) = line.strip_prefix("DEPRECATED:") {
            let rest = rest.trim();
            if let Some(arrow_pos) = rest.find("->") {
                let old_path = rest[..arrow_pos].trim().to_string();
                let new_path = rest[arrow_pos + 2..].trim().to_string();
                if !old_path.is_empty() && !new_path.is_empty() {
                    notes.push(MigrationNote {
                        old_path,
                        new_path: Some(new_path),
                        since_version: None,
                        source: MigrationSource::AiDetected,
                    });
                }
            }
        }
    }

    notes
}

/// Check if a dotted path exists in a `toml::Value`.
fn field_exists(value: &toml::Value, dotted_path: &str) -> bool {
    let mut current = value;
    for part in dotted_path.split('.') {
        match current.get(part) {
            Some(v) => current = v,
            None => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn test_parse_migration_none() {
        let notes = parse_migration_output("NONE");
        assert!(notes.is_empty());
    }

    #[test]
    fn test_parse_migration_deprecated() {
        let output =
            "DEPRECATED: gateway.bind -> gateway.bind_host\nDEPRECATED: foo.bar -> foo.baz\n";
        let notes = parse_migration_output(output);
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].old_path, "gateway.bind");
        assert_eq!(notes[0].new_path.as_deref(), Some("gateway.bind_host"));
        assert_eq!(notes[0].source, MigrationSource::AiDetected);
    }

    #[test]
    fn test_format_known_renames() {
        let s = format_known_renames();
        assert!(s.contains("agents.list"));
        assert!(s.contains("vault:agents"));
    }

    #[test]
    fn test_field_exists() {
        let toml: toml::Value = "[gateway]\nport = 8080\n".parse().unwrap();
        assert!(field_exists(&toml, "gateway.port"));
        assert!(!field_exists(&toml, "gateway.missing"));
        assert!(!field_exists(&toml, "other.field"));
    }

    #[test]
    fn test_check_static_table_no_matches() {
        let toml = "[gateway]\nport = 8080\n";
        let notes = check_static_table(toml);
        assert!(notes.is_empty());
    }

    #[test]
    fn test_check_static_table_agents_list() {
        let toml = "[agents]\nlist = []\n";
        let notes = check_static_table(toml);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].old_path, "agents.list");
        assert_eq!(notes[0].new_path.as_deref(), Some("vault:agents"));
        assert_eq!(notes[0].source, MigrationSource::StaticTable);
    }
}
