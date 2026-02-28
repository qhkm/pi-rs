use super::traits::AgentTool;
use std::collections::HashMap;
use std::sync::Arc;

/// Registry of tools available to the agent
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn AgentTool>>,
    /// Tools that are currently active (subset of all registered)
    active_tools: Vec<String>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            active_tools: Vec::new(),
        }
    }

    /// Register a tool. Replaces any existing tool with the same name.
    pub fn register(&mut self, tool: Arc<dyn AgentTool>) {
        let name = tool.name().to_string();
        self.active_tools.push(name.clone());
        self.tools.insert(name, tool);
    }

    /// Unregister a tool by name
    pub fn unregister(&mut self, name: &str) -> Option<Arc<dyn AgentTool>> {
        self.active_tools.retain(|n| n != name);
        self.tools.remove(name)
    }

    /// Get a tool by name
    pub fn get(&self, name: &str) -> Option<&Arc<dyn AgentTool>> {
        self.tools.get(name)
    }

    /// Get all registered tools
    pub fn all(&self) -> Vec<&Arc<dyn AgentTool>> {
        self.tools.values().collect()
    }

    /// Get only active tools (tools currently available to the agent)
    pub fn active(&self) -> Vec<&Arc<dyn AgentTool>> {
        self.active_tools
            .iter()
            .filter_map(|name| self.tools.get(name))
            .collect()
    }

    /// Set which tools are active by name
    pub fn set_active(&mut self, names: Vec<String>) {
        self.active_tools = names;
    }

    /// Activate a specific tool
    pub fn activate(&mut self, name: &str) {
        if self.tools.contains_key(name) && !self.active_tools.contains(&name.to_string()) {
            self.active_tools.push(name.to_string());
        }
    }

    /// Deactivate a specific tool
    pub fn deactivate(&mut self, name: &str) {
        self.active_tools.retain(|n| n != name);
    }

    /// Get tool definitions for active tools (to send to LLM)
    pub fn active_tool_definitions(&self) -> Vec<pi_ai::ToolDefinition> {
        self.active()
            .iter()
            .map(|t| t.to_tool_definition())
            .collect()
    }

    /// Check if a tool exists
    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Number of registered tools
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Whether the registry is empty
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
