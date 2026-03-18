use std::sync::Arc;

use rockbot_tools::{builtin, Result, Tool, ToolConfig, ToolRegistry};

pub const MINIMAL_SYSTEM_TOOLS: &[&str] = &["read", "write"];
pub const STANDARD_SYSTEM_TOOLS: &[&str] = &[
    "read",
    "write",
    "edit",
    "exec",
    "glob",
    "grep",
    "patch",
    "web_fetch",
    "web_search",
    "browser",
    "test",
    "lint",
];
pub const FULL_SYSTEM_TOOLS: &[&str] = STANDARD_SYSTEM_TOOLS;

pub async fn register_profile_tools(registry: &ToolRegistry, config: &ToolConfig) -> Result<()> {
    let tool_names: &[&str] = match config.profile.as_str() {
        "minimal" => MINIMAL_SYSTEM_TOOLS,
        "standard" => STANDARD_SYSTEM_TOOLS,
        "full" => FULL_SYSTEM_TOOLS,
        _ => &["read", "write", "edit", "exec", "glob", "grep", "patch"],
    };

    for tool_name in tool_names {
        if config.deny.iter().any(|denied| denied == tool_name) {
            continue;
        }

        if let Some(tool) = create_system_tool(tool_name) {
            registry.register_tool(tool).await;
        }
    }

    Ok(())
}

pub fn create_system_tool(name: &str) -> Option<Arc<dyn Tool>> {
    match name {
        "read" => Some(Arc::new(builtin::ReadTool::new())),
        "write" => Some(Arc::new(builtin::WriteTool::new())),
        "edit" => Some(Arc::new(builtin::EditTool::new())),
        "exec" => Some(Arc::new(builtin::ExecTool::new())),
        "glob" => Some(Arc::new(builtin::GlobTool::new())),
        "grep" => Some(Arc::new(builtin::GrepTool::new())),
        "patch" => Some(Arc::new(builtin::PatchTool::new())),
        "web_fetch" => Some(Arc::new(builtin::WebFetchTool::new())),
        "web_search" => Some(Arc::new(builtin::WebSearchTool::new())),
        "browser" => Some(Arc::new(builtin::BrowserTool::new())),
        "test" => Some(Arc::new(builtin::TestTool::new())),
        "lint" => Some(Arc::new(builtin::LintTool::new())),
        _ => None,
    }
}
