//! Plugin system for Krabbykrus
//! 
//! This module provides the plugin loading and management system.
//! In the full implementation, this would support WebAssembly plugins.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

/// Plugin system errors
#[derive(Debug, Error)]
pub enum PluginError {
    #[error("Plugin not found: {name}")]
    NotFound { name: String },
    
    #[error("Plugin loading failed: {message}")]
    LoadingFailed { message: String },
    
    #[error("Plugin execution failed: {message}")]
    ExecutionFailed { message: String },
    
    #[error("Invalid plugin manifest: {message}")]
    InvalidManifest { message: String },
    
    #[error("Security error: {message}")]
    SecurityError { message: String },
}

/// Result type for plugin operations
pub type Result<T> = std::result::Result<T, PluginError>;

/// Plugin manager handles plugin lifecycle
pub struct PluginManager {
    plugins: HashMap<String, LoadedPlugin>,
}

/// A loaded plugin
#[derive(Debug)]
pub struct LoadedPlugin {
    pub id: String,
    pub manifest: PluginManifest,
    pub state: PluginState,
}

/// Plugin manifest describes a plugin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub capabilities: Vec<String>,
    pub tools: Vec<PluginToolDefinition>,
    pub channels: Vec<PluginChannelDefinition>,
}

/// Plugin-provided tool definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Plugin-provided channel definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginChannelDefinition {
    pub name: String,
    pub protocol: String,
    pub description: String,
}

/// Plugin execution state
#[derive(Debug, Clone)]
pub enum PluginState {
    Loaded,
    Running,
    Stopped,
    Error { message: String },
}

/// Plugin context for execution
#[derive(Debug, Clone)]
pub struct PluginContext {
    pub plugin_id: String,
    pub config: serde_json::Value,
    pub capabilities: Vec<String>,
}

impl PluginManager {
    /// Create a new plugin manager
    pub fn new() -> Self {
        Self {
            plugins: HashMap::new(),
        }
    }
    
    /// Load a plugin from a manifest
    pub async fn load_plugin(&mut self, manifest: PluginManifest) -> Result<()> {
        tracing::info!("Loading plugin: {} v{}", manifest.name, manifest.version);
        
        let plugin = LoadedPlugin {
            id: manifest.id.clone(),
            manifest,
            state: PluginState::Loaded,
        };
        
        self.plugins.insert(plugin.id.clone(), plugin);
        
        Ok(())
    }
    
    /// Unload a plugin
    pub async fn unload_plugin(&mut self, plugin_id: &str) -> Result<()> {
        if let Some(plugin) = self.plugins.remove(plugin_id) {
            tracing::info!("Unloaded plugin: {}", plugin.manifest.name);
            Ok(())
        } else {
            Err(PluginError::NotFound {
                name: plugin_id.to_string(),
            })
        }
    }
    
    /// Get plugin by ID
    pub fn get_plugin(&self, plugin_id: &str) -> Option<&LoadedPlugin> {
        self.plugins.get(plugin_id)
    }
    
    /// List all loaded plugins
    pub fn list_plugins(&self) -> Vec<&LoadedPlugin> {
        self.plugins.values().collect()
    }
    
    /// Get tools provided by all plugins
    pub fn get_plugin_tools(&self) -> Vec<PluginToolDefinition> {
        let mut tools = Vec::new();
        for plugin in self.plugins.values() {
            tools.extend(plugin.manifest.tools.clone());
        }
        tools
    }
    
    /// Get channels provided by all plugins
    pub fn get_plugin_channels(&self) -> Vec<PluginChannelDefinition> {
        let mut channels = Vec::new();
        for plugin in self.plugins.values() {
            channels.extend(plugin.manifest.channels.clone());
        }
        channels
    }
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_plugin_loading() {
        let mut manager = PluginManager::new();
        
        let manifest = PluginManifest {
            id: "test-plugin".to_string(),
            name: "Test Plugin".to_string(),
            version: "1.0.0".to_string(),
            description: "A test plugin".to_string(),
            author: "Test Author".to_string(),
            capabilities: vec!["filesystem".to_string()],
            tools: vec![],
            channels: vec![],
        };
        
        manager.load_plugin(manifest).await.unwrap();
        
        assert!(manager.get_plugin("test-plugin").is_some());
        assert_eq!(manager.list_plugins().len(), 1);
    }
}