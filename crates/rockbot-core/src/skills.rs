//! Skills system for RockBot (SPEC Section 16)
//!
//! This module provides skill discovery, loading, filtering, and context injection.
//! Skills are modular capability definitions that extend agent behavior by providing
//! prompt instructions, metadata, and installation specifications.

use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// A skill definition containing instructions, metadata, and install specs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    /// Unique skill name
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// The skill prompt/instructions injected into the system prompt
    pub content: String,
    /// Optional metadata controlling behavior and requirements
    #[serde(default)]
    pub metadata: Option<SkillMetadata>,
    /// Installation specifications for required dependencies
    #[serde(default)]
    pub install: Vec<InstallSpec>,
}

/// Metadata governing how a skill is activated and what it requires.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMetadata {
    /// Always include this skill in context (no explicit invocation needed)
    #[serde(default)]
    pub always: bool,
    /// Unique key for referencing this skill programmatically
    #[serde(default)]
    pub skill_key: Option<String>,
    /// Emoji icon for display purposes
    #[serde(default)]
    pub emoji: Option<String>,
    /// URL to skill documentation or homepage
    #[serde(default)]
    pub homepage: Option<String>,
    /// Supported operating systems (empty = all)
    #[serde(default)]
    pub os: Vec<String>,
    /// Requirements that must be met for this skill to be available
    #[serde(default)]
    pub requires: SkillRequirements,
}

/// Requirements that must be satisfied before a skill can be used.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillRequirements {
    /// Required binaries that must be on PATH
    #[serde(default)]
    pub bins: Vec<String>,
    /// Required environment variables that must be set
    #[serde(default)]
    pub env: Vec<String>,
    /// Required configuration keys
    #[serde(default)]
    pub config: Vec<String>,
}

/// Specification for installing a skill dependency.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallSpec {
    /// The type of installer to use
    pub kind: InstallKind,
    /// Human-readable label for this install option
    #[serde(default)]
    pub label: Option<String>,
    /// Homebrew formula name (for Brew kind)
    #[serde(default)]
    pub formula: Option<String>,
    /// Package name (for Node/Go kind)
    #[serde(default)]
    pub package: Option<String>,
    /// Download URL (for Download kind)
    #[serde(default)]
    pub url: Option<String>,
    /// OS restrictions for this install method (empty = all)
    #[serde(default)]
    pub os: Vec<String>,
}

/// The kind of package manager or install mechanism.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum InstallKind {
    Brew,
    Node,
    Go,
    Uv,
    Download,
}

/// Policy controlling how a skill can be invoked.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInvocationPolicy {
    /// Whether the user can invoke this skill directly (e.g., via slash command)
    #[serde(default = "default_true")]
    pub user_invocable: bool,
    /// Whether the model is prevented from invoking this skill on its own
    #[serde(default)]
    pub disable_model_invocation: bool,
}

impl Default for SkillInvocationPolicy {
    fn default() -> Self {
        Self {
            user_invocable: true,
            disable_model_invocation: false,
        }
    }
}

fn default_true() -> bool {
    true
}

/// Source from which a skill was discovered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillSource {
    /// Bundled with the package
    Bundled,
    /// From workspace configuration
    Workspace,
    /// Configured per-agent
    Agent(String),
}

/// A loaded skill with its source provenance.
#[derive(Debug, Clone)]
pub struct LoadedSkill {
    pub skill: Skill,
    pub source: SkillSource,
    pub policy: SkillInvocationPolicy,
    /// Whether all requirements are currently satisfied
    pub requirements_met: bool,
}

/// Manages skill discovery, loading, filtering, and context assembly.
pub struct SkillManager {
    /// All discovered and loaded skills, keyed by name
    skills: HashMap<String, LoadedSkill>,
    /// Bundled skills directory
    bundled_dir: Option<PathBuf>,
    /// Workspace skill paths from config
    workspace_paths: Vec<PathBuf>,
    /// Per-agent skill paths
    agent_paths: HashMap<String, Vec<PathBuf>>,
    /// Available config keys (for requirement checking)
    config_keys: Vec<String>,
}

impl SkillManager {
    /// Create a new SkillManager.
    ///
    /// - `package_root`: Root directory of the package (for bundled skills)
    /// - `workspace_paths`: Additional skill directories from workspace config
    pub fn new(package_root: Option<&Path>, workspace_paths: Vec<PathBuf>) -> Self {
        let bundled_dir = package_root.map(|p| p.join("skills"));
        Self {
            skills: HashMap::new(),
            bundled_dir,
            workspace_paths,
            agent_paths: HashMap::new(),
            config_keys: Vec::new(),
        }
    }

    /// Register agent-specific skill paths.
    pub fn add_agent_skill_paths(&mut self, agent_id: &str, paths: Vec<PathBuf>) {
        self.agent_paths.insert(agent_id.to_string(), paths);
    }

    /// Set available config keys for requirement checking.
    pub fn set_config_keys(&mut self, keys: Vec<String>) {
        self.config_keys = keys;
    }

    /// Discover and load all skills from all sources.
    ///
    /// Discovery order (later sources can override earlier ones):
    /// 1. Bundled: `{package_root}/skills/`
    /// 2. Workspace: from config-specified paths
    /// 3. Agent-specific: per-agent skill configuration
    pub async fn discover_all(&mut self) -> Result<usize> {
        let mut count = 0;

        // 1. Bundled skills
        if let Some(ref dir) = self.bundled_dir.clone() {
            count += self.load_skills_from_dir(dir, SkillSource::Bundled).await?;
        }

        // 2. Workspace skills
        for path in self.workspace_paths.clone() {
            count += self.load_skills_from_dir(&path, SkillSource::Workspace).await?;
        }

        // 3. Agent-specific skills
        for (agent_id, paths) in self.agent_paths.clone() {
            for path in &paths {
                count += self
                    .load_skills_from_dir(path, SkillSource::Agent(agent_id.clone()))
                    .await?;
            }
        }

        // Check requirements for all loaded skills
        self.check_all_requirements();

        info!("Discovered {} skills total", count);
        Ok(count)
    }

    /// Load skill definitions from a directory. Each `.toml` file is one skill.
    async fn load_skills_from_dir(&mut self, dir: &Path, source: SkillSource) -> Result<usize> {
        let mut count = 0;

        let entries = match tokio::fs::read_dir(dir).await {
            Ok(entries) => entries,
            Err(e) => {
                debug!("Skill directory not accessible {}: {}", dir.display(), e);
                return Ok(0);
            }
        };

        let mut entries = entries;
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("toml") {
                match self.load_skill_file(&path, source.clone()).await {
                    Ok(()) => count += 1,
                    Err(e) => {
                        warn!("Failed to load skill from {}: {}", path.display(), e);
                    }
                }
            }
        }

        debug!(
            "Loaded {} skills from {} ({:?})",
            count,
            dir.display(),
            source
        );
        Ok(count)
    }

    /// Load a single skill definition file.
    async fn load_skill_file(&mut self, path: &Path, source: SkillSource) -> Result<()> {
        let content = tokio::fs::read_to_string(path).await?;
        let skill: Skill = toml::from_str(&content)?;

        let name = skill.name.clone();
        let loaded = LoadedSkill {
            skill,
            source,
            policy: SkillInvocationPolicy::default(),
            requirements_met: false, // Will be checked after all loading
        };

        debug!("Loaded skill '{}' from {}", name, path.display());
        self.skills.insert(name, loaded);
        Ok(())
    }

    /// Load a skill directly from a Skill struct (useful for programmatic registration).
    pub fn register_skill(&mut self, skill: Skill, source: SkillSource, policy: SkillInvocationPolicy) {
        let name = skill.name.clone();
        let mut loaded = LoadedSkill {
            skill,
            source,
            policy,
            requirements_met: false,
        };
        loaded.requirements_met = self.check_requirements(&loaded.skill);
        self.skills.insert(name, loaded);
    }

    /// Check requirements for all loaded skills and update their status.
    fn check_all_requirements(&mut self) {
        let config_keys = self.config_keys.clone();
        for loaded in self.skills.values_mut() {
            loaded.requirements_met = check_skill_requirements(&loaded.skill, &config_keys);
        }
    }

    /// Check if a single skill's requirements are met.
    fn check_requirements(&self, skill: &Skill) -> bool {
        check_skill_requirements(skill, &self.config_keys)
    }

    /// Get all skills that are available for a given agent and OS.
    ///
    /// A skill is available when:
    /// - Its OS filter matches (or is empty, meaning all)
    /// - All requirements are met
    /// - It belongs to the bundled/workspace scope, or is configured for this agent
    pub fn available_skills(&self, agent_id: &str, current_os: &str) -> Vec<&LoadedSkill> {
        self.skills
            .values()
            .filter(|ls| {
                // OS filter
                if !ls.skill.metadata.as_ref().map_or(true, |m| {
                    m.os.is_empty() || m.os.iter().any(|o| o.eq_ignore_ascii_case(current_os))
                }) {
                    return false;
                }

                // Requirements check
                if !ls.requirements_met {
                    return false;
                }

                // Source scope check
                match &ls.source {
                    SkillSource::Bundled | SkillSource::Workspace => true,
                    SkillSource::Agent(id) => id == agent_id,
                }
            })
            .collect()
    }

    /// Get skills that should always be included in the system prompt.
    pub fn always_on_skills(&self, agent_id: &str, current_os: &str) -> Vec<&LoadedSkill> {
        self.available_skills(agent_id, current_os)
            .into_iter()
            .filter(|ls| {
                ls.skill
                    .metadata
                    .as_ref()
                    .map_or(false, |m| m.always)
            })
            .collect()
    }

    /// Get a specific skill by name, if available for the given agent.
    pub fn get_skill(&self, name: &str) -> Option<&LoadedSkill> {
        self.skills.get(name)
    }

    /// Get a skill by its skill_key (from metadata).
    pub fn get_skill_by_key(&self, key: &str) -> Option<&LoadedSkill> {
        self.skills.values().find(|ls| {
            ls.skill
                .metadata
                .as_ref()
                .and_then(|m| m.skill_key.as_deref())
                == Some(key)
        })
    }

    /// Assemble skill context for injection into the system prompt.
    ///
    /// This gathers all always-on skills plus any explicitly requested skills,
    /// and formats their content for system prompt inclusion.
    pub fn assemble_skill_context(
        &self,
        agent_id: &str,
        current_os: &str,
        additional_skill_names: &[String],
    ) -> String {
        let mut parts = Vec::new();

        // Collect always-on skills
        let always_on = self.always_on_skills(agent_id, current_os);
        for ls in &always_on {
            parts.push(format_skill_section(&ls.skill));
        }

        // Collect explicitly requested skills (avoid duplicates with always-on)
        let always_on_names: Vec<&str> = always_on.iter().map(|ls| ls.skill.name.as_str()).collect();
        for name in additional_skill_names {
            if always_on_names.contains(&name.as_str()) {
                continue;
            }
            if let Some(ls) = self.skills.get(name) {
                if ls.requirements_met {
                    parts.push(format_skill_section(&ls.skill));
                } else {
                    debug!(
                        "Skill '{}' requested but requirements not met, skipping",
                        name
                    );
                }
            }
        }

        if parts.is_empty() {
            return String::new();
        }

        let mut output = String::from("# Skills\n\n");
        output.push_str(&parts.join("\n\n---\n\n"));
        output
    }

    /// List all loaded skill names and their availability status.
    pub fn list_skills(&self) -> Vec<SkillSummary> {
        self.skills
            .values()
            .map(|ls| SkillSummary {
                name: ls.skill.name.clone(),
                description: ls.skill.description.clone(),
                source: ls.source.clone(),
                requirements_met: ls.requirements_met,
                always_on: ls
                    .skill
                    .metadata
                    .as_ref()
                    .map_or(false, |m| m.always),
                emoji: ls
                    .skill
                    .metadata
                    .as_ref()
                    .and_then(|m| m.emoji.clone()),
            })
            .collect()
    }
}

/// Summary information about a skill for listing/display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
    #[serde(skip)]
    pub source: SkillSource,
    pub requirements_met: bool,
    pub always_on: bool,
    pub emoji: Option<String>,
}

impl Default for SkillSource {
    fn default() -> Self {
        Self::Bundled
    }
}

/// Check whether a skill's requirements are satisfied on the current system.
fn check_skill_requirements(skill: &Skill, config_keys: &[String]) -> bool {
    let reqs = match skill.metadata.as_ref().map(|m| &m.requires) {
        Some(reqs) => reqs,
        None => return true, // No requirements means always available
    };

    // Check required binaries
    for bin in &reqs.bins {
        if !is_binary_available(bin) {
            debug!(
                "Skill '{}' requirement not met: binary '{}' not found",
                skill.name, bin
            );
            return false;
        }
    }

    // Check required environment variables
    for var in &reqs.env {
        if std::env::var(var).is_err() {
            debug!(
                "Skill '{}' requirement not met: env var '{}' not set",
                skill.name, var
            );
            return false;
        }
    }

    // Check required config keys
    for key in &reqs.config {
        if !config_keys.contains(key) {
            debug!(
                "Skill '{}' requirement not met: config key '{}' not present",
                skill.name, key
            );
            return false;
        }
    }

    true
}

/// Check if a binary is available on PATH.
fn is_binary_available(name: &str) -> bool {
    which::which(name).is_ok()
}

/// Format a skill's content for inclusion in the system prompt.
fn format_skill_section(skill: &Skill) -> String {
    let emoji = skill
        .metadata
        .as_ref()
        .and_then(|m| m.emoji.as_deref())
        .unwrap_or("");

    let header = if emoji.is_empty() {
        format!("## Skill: {}", skill.name)
    } else {
        format!("## {} Skill: {}", emoji, skill.name)
    };

    format!("{}\n\n{}\n\n{}", header, skill.description, skill.content)
}

/// Detect the current operating system as a lowercase string.
pub fn current_os() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "unknown"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_skill(name: &str, always: bool) -> Skill {
        Skill {
            name: name.to_string(),
            description: format!("{} skill", name),
            content: format!("Instructions for {}", name),
            metadata: Some(SkillMetadata {
                always,
                skill_key: Some(name.to_lowercase()),
                emoji: Some("*".to_string()),
                homepage: None,
                os: vec![],
                requires: SkillRequirements::default(),
            }),
            install: vec![],
        }
    }

    #[test]
    fn test_skill_serialization_roundtrip() {
        let skill = make_skill("test-skill", true);
        let toml_str = toml::to_string_pretty(&skill).unwrap();
        let deserialized: Skill = toml::from_str(&toml_str).unwrap();
        assert_eq!(deserialized.name, "test-skill");
        assert!(deserialized.metadata.as_ref().unwrap().always);
    }

    #[test]
    fn test_install_kind_serialization() {
        let spec = InstallSpec {
            kind: InstallKind::Brew,
            label: Some("Install via Homebrew".to_string()),
            formula: Some("my-tool".to_string()),
            package: None,
            url: None,
            os: vec!["macos".to_string()],
        };
        let toml_str = toml::to_string_pretty(&spec).unwrap();
        assert!(toml_str.contains("brew"));
        let deserialized: InstallSpec = toml::from_str(&toml_str).unwrap();
        assert_eq!(deserialized.kind, InstallKind::Brew);
    }

    #[test]
    fn test_requirements_no_metadata_always_met() {
        let skill = Skill {
            name: "simple".to_string(),
            description: "A simple skill".to_string(),
            content: "Do the thing".to_string(),
            metadata: None,
            install: vec![],
        };
        assert!(check_skill_requirements(&skill, &[]));
    }

    #[test]
    fn test_requirements_env_var_check() {
        std::env::set_var("ROCKBOT_TEST_SKILL_VAR", "1");
        let skill = Skill {
            name: "env-skill".to_string(),
            description: "Needs env".to_string(),
            content: "content".to_string(),
            metadata: Some(SkillMetadata {
                always: false,
                skill_key: None,
                emoji: None,
                homepage: None,
                os: vec![],
                requires: SkillRequirements {
                    bins: vec![],
                    env: vec!["ROCKBOT_TEST_SKILL_VAR".to_string()],
                    config: vec![],
                },
            }),
            install: vec![],
        };
        assert!(check_skill_requirements(&skill, &[]));

        // Missing env var
        let skill_missing = Skill {
            name: "env-skill-missing".to_string(),
            description: "Needs env".to_string(),
            content: "content".to_string(),
            metadata: Some(SkillMetadata {
                always: false,
                skill_key: None,
                emoji: None,
                homepage: None,
                os: vec![],
                requires: SkillRequirements {
                    bins: vec![],
                    env: vec!["ROCKBOT_DOES_NOT_EXIST_XYZ".to_string()],
                    config: vec![],
                },
            }),
            install: vec![],
        };
        assert!(!check_skill_requirements(&skill_missing, &[]));
    }

    #[test]
    fn test_requirements_config_key_check() {
        let skill = Skill {
            name: "config-skill".to_string(),
            description: "Needs config".to_string(),
            content: "content".to_string(),
            metadata: Some(SkillMetadata {
                always: false,
                skill_key: None,
                emoji: None,
                homepage: None,
                os: vec![],
                requires: SkillRequirements {
                    bins: vec![],
                    env: vec![],
                    config: vec!["providers.openai".to_string()],
                },
            }),
            install: vec![],
        };
        assert!(!check_skill_requirements(&skill, &[]));
        assert!(check_skill_requirements(
            &skill,
            &["providers.openai".to_string()]
        ));
    }

    #[test]
    fn test_register_and_retrieve_skill() {
        let mut manager = SkillManager::new(None, vec![]);
        let skill = make_skill("my-skill", false);
        manager.register_skill(skill, SkillSource::Bundled, SkillInvocationPolicy::default());

        assert!(manager.get_skill("my-skill").is_some());
        assert!(manager.get_skill("nonexistent").is_none());
    }

    #[test]
    fn test_get_skill_by_key() {
        let mut manager = SkillManager::new(None, vec![]);
        let skill = make_skill("My-Skill", false);
        manager.register_skill(skill, SkillSource::Bundled, SkillInvocationPolicy::default());

        assert!(manager.get_skill_by_key("my-skill").is_some());
        assert!(manager.get_skill_by_key("nope").is_none());
    }

    #[test]
    fn test_available_skills_os_filter() {
        let mut manager = SkillManager::new(None, vec![]);

        let mut linux_skill = make_skill("linux-only", false);
        linux_skill.metadata.as_mut().unwrap().os = vec!["linux".to_string()];
        manager.register_skill(
            linux_skill,
            SkillSource::Bundled,
            SkillInvocationPolicy::default(),
        );

        let mut macos_skill = make_skill("macos-only", false);
        macos_skill.metadata.as_mut().unwrap().os = vec!["macos".to_string()];
        manager.register_skill(
            macos_skill,
            SkillSource::Bundled,
            SkillInvocationPolicy::default(),
        );

        let linux_avail = manager.available_skills("agent-1", "linux");
        assert!(linux_avail.iter().any(|s| s.skill.name == "linux-only"));
        assert!(!linux_avail.iter().any(|s| s.skill.name == "macos-only"));
    }

    #[test]
    fn test_available_skills_agent_scope() {
        let mut manager = SkillManager::new(None, vec![]);

        let skill = make_skill("agent-specific", false);
        manager.register_skill(
            skill,
            SkillSource::Agent("agent-a".to_string()),
            SkillInvocationPolicy::default(),
        );

        assert_eq!(manager.available_skills("agent-a", "linux").len(), 1);
        assert_eq!(manager.available_skills("agent-b", "linux").len(), 0);
    }

    #[test]
    fn test_always_on_skills() {
        let mut manager = SkillManager::new(None, vec![]);

        manager.register_skill(
            make_skill("always", true),
            SkillSource::Bundled,
            SkillInvocationPolicy::default(),
        );
        manager.register_skill(
            make_skill("on-demand", false),
            SkillSource::Bundled,
            SkillInvocationPolicy::default(),
        );

        let always = manager.always_on_skills("agent-1", "linux");
        assert_eq!(always.len(), 1);
        assert_eq!(always[0].skill.name, "always");
    }

    #[test]
    fn test_assemble_skill_context() {
        let mut manager = SkillManager::new(None, vec![]);
        manager.register_skill(
            make_skill("always-skill", true),
            SkillSource::Bundled,
            SkillInvocationPolicy::default(),
        );
        manager.register_skill(
            make_skill("extra-skill", false),
            SkillSource::Bundled,
            SkillInvocationPolicy::default(),
        );

        let context = manager.assemble_skill_context(
            "agent-1",
            "linux",
            &["extra-skill".to_string()],
        );

        assert!(context.contains("# Skills"));
        assert!(context.contains("always-skill"));
        assert!(context.contains("extra-skill"));
    }

    #[test]
    fn test_assemble_skill_context_empty() {
        let manager = SkillManager::new(None, vec![]);
        let context = manager.assemble_skill_context("agent-1", "linux", &[]);
        assert!(context.is_empty());
    }

    #[test]
    fn test_list_skills() {
        let mut manager = SkillManager::new(None, vec![]);
        manager.register_skill(
            make_skill("skill-a", true),
            SkillSource::Bundled,
            SkillInvocationPolicy::default(),
        );
        manager.register_skill(
            make_skill("skill-b", false),
            SkillSource::Workspace,
            SkillInvocationPolicy::default(),
        );

        let summaries = manager.list_skills();
        assert_eq!(summaries.len(), 2);
    }

    #[test]
    fn test_skill_invocation_policy_defaults() {
        let policy = SkillInvocationPolicy::default();
        assert!(policy.user_invocable);
        assert!(!policy.disable_model_invocation);
    }

    #[test]
    fn test_format_skill_section() {
        let skill = make_skill("test", false);
        let section = format_skill_section(&skill);
        assert!(section.contains("Skill: test"));
        assert!(section.contains("test skill"));
        assert!(section.contains("Instructions for test"));
    }

    #[tokio::test]
    async fn test_discover_from_directory() {
        let dir = tempfile::tempdir().unwrap();
        let skill_path = dir.path().join("hello.toml");
        let skill_toml = r#"
name = "hello"
description = "A greeting skill"
content = "Say hello to the user"
"#;
        tokio::fs::write(&skill_path, skill_toml).await.unwrap();

        let mut manager = SkillManager::new(None, vec![dir.path().to_path_buf()]);
        let count = manager.discover_all().await.unwrap();
        assert_eq!(count, 1);
        assert!(manager.get_skill("hello").is_some());
    }

    #[tokio::test]
    async fn test_discover_skips_missing_directory() {
        let mut manager = SkillManager::new(None, vec![PathBuf::from("/nonexistent/path")]);
        let count = manager.discover_all().await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_discover_bundled_directory() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        tokio::fs::create_dir_all(&skills_dir).await.unwrap();

        let skill_toml = r#"
name = "bundled-skill"
description = "A bundled skill"
content = "Bundled instructions"
"#;
        tokio::fs::write(skills_dir.join("bundled.toml"), skill_toml)
            .await
            .unwrap();

        let mut manager = SkillManager::new(Some(dir.path()), vec![]);
        let count = manager.discover_all().await.unwrap();
        assert_eq!(count, 1);

        let loaded = manager.get_skill("bundled-skill").unwrap();
        assert_eq!(loaded.source, SkillSource::Bundled);
    }
}
