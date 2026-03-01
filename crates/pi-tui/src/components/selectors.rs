use crate::components::select_list::{SelectItem, SelectList, SelectListTheme};
use crate::components::traits::{Component, Focusable, InputResult};
use crate::keyboard::keybindings::EditorAction;

/// A model information entry for the model selector.
#[derive(Debug, Clone)]
pub struct ModelInfo {
    /// Model ID (used for selection)
    pub id: String,
    /// Display name
    pub name: String,
    /// Provider name (e.g., "Anthropic", "OpenAI")
    pub provider: String,
    /// Context window size
    pub context_window: Option<usize>,
    /// Cost per 1M input tokens
    pub input_cost: Option<f64>,
    /// Cost per 1M output tokens
    pub output_cost: Option<f64>,
    /// Capabilities
    pub capabilities: Vec<String>,
}

impl ModelInfo {
    /// Create a new model info.
    pub fn new(id: impl Into<String>, name: impl Into<String>, provider: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            provider: provider.into(),
            context_window: None,
            input_cost: None,
            output_cost: None,
            capabilities: Vec::new(),
        }
    }

    /// Set context window.
    pub fn with_context_window(mut self, window: usize) -> Self {
        self.context_window = Some(window);
        self
    }

    /// Set costs.
    pub fn with_costs(mut self, input: f64, output: f64) -> Self {
        self.input_cost = Some(input);
        self.output_cost = Some(output);
        self
    }

    /// Set capabilities.
    pub fn with_capabilities(mut self, caps: Vec<String>) -> Self {
        self.capabilities = caps;
        self
    }

    /// Format as a display string.
    pub fn display_label(&self) -> String {
        format!("{} ({})", self.name, self.provider)
    }

    /// Format description with details.
    pub fn display_description(&self) -> String {
        let mut parts = Vec::new();
        
        if let Some(window) = self.context_window {
            parts.push(format!("{}K context", window / 1000));
        }
        
        if let (Some(input), Some(output)) = (self.input_cost, self.output_cost) {
            parts.push(format!("${:.2}/${:.2}", input, output));
        }
        
        if !self.capabilities.is_empty() {
            parts.push(self.capabilities.join(", "));
        }
        
        parts.join(" • ")
    }
}

/// Model selector component.
///
/// A specialized SelectList for choosing AI models with rich information display.
pub struct ModelSelector {
    models: Vec<ModelInfo>,
    selected_id: Option<String>,
    select_list: SelectList,
    filter_provider: Option<String>,
}

impl ModelSelector {
    /// Create a new model selector.
    pub fn new(models: Vec<ModelInfo>) -> Self {
        let items: Vec<SelectItem> = models
            .iter()
            .map(|m| {
                SelectItem::new(m.id.clone(), m.display_label())
                    .with_description(m.display_description())
            })
            .collect();

        let theme = SelectListTheme::default();
        let select_list = SelectList::new(items, 10).with_theme(theme);

        Self {
            models,
            selected_id: None,
            select_list,
            filter_provider: None,
        }
    }

    /// Set the selected model by ID.
    pub fn set_selected(&mut self, id: &str) {
        self.selected_id = Some(id.to_string());
        if let Some(idx) = self.models.iter().position(|m| m.id == id) {
            // Update select_list selection - this would need to be added to SelectList
        }
    }

    /// Get the selected model ID.
    pub fn selected_id(&self) -> Option<&str> {
        self.selected_id.as_deref()
    }

    /// Get the selected model info.
    pub fn selected_model(&self) -> Option<&ModelInfo> {
        self.selected_id
            .as_ref()
            .and_then(|id| self.models.iter().find(|m| m.id == *id))
    }

    /// Filter by provider.
    pub fn filter_by_provider(&mut self, provider: Option<String>) {
        self.filter_provider = provider;
        self.update_items();
    }

    fn update_items(&mut self) {
        let items: Vec<SelectItem> = self
            .models
            .iter()
            .filter(|m| {
                self.filter_provider
                    .as_ref()
                    .map(|p| m.provider == *p)
                    .unwrap_or(true)
            })
            .map(|m| {
                SelectItem::new(m.id.clone(), m.display_label())
                    .with_description(m.display_description())
            })
            .collect();

        self.select_list.set_items(items);
    }

    /// Handle selection confirmation.
    pub fn confirm_selection(&mut self) {
        if let Some(item) = self.select_list.selected_item() {
            self.selected_id = Some(item.value.clone());
        }
    }

    /// Set on select callback.
    pub fn on_select<F>(&mut self, callback: F)
    where
        F: Fn(&ModelInfo) + Send + 'static,
    {
        let models = self.models.clone();
        self.select_list.on_select = Some(Box::new(move |item| {
            if let Some(model) = models.iter().find(|m| m.id == item.value) {
                callback(model);
            }
        }));
    }
}

impl Component for ModelSelector {
    fn render(&self, width: u16) -> Vec<String> {
        self.select_list.render(width)
    }

    fn handle_input(&mut self, data: &str) -> InputResult {
        self.select_list.handle_input(data)
    }

    fn invalidate(&mut self) {
        self.select_list.invalidate();
    }

    fn is_dirty(&self) -> bool {
        self.select_list.is_dirty()
    }
}

impl Focusable for ModelSelector {
    fn focused(&self) -> bool {
        self.select_list.focused()
    }

    fn set_focused(&mut self, focused: bool) {
        self.select_list.set_focused(focused);
    }
}

/// Thinking level definition.
#[derive(Debug, Clone)]
pub struct ThinkingLevel {
    /// Level identifier
    pub id: String,
    /// Display name
    pub name: String,
    /// Description
    pub description: String,
    /// Budget in tokens (if applicable)
    pub budget: Option<usize>,
    /// Color indicator
    pub color: Option<String>,
}

impl ThinkingLevel {
    /// Create a new thinking level.
    pub fn new(id: impl Into<String>, name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: description.into(),
            budget: None,
            color: None,
        }
    }

    /// Set budget.
    pub fn with_budget(mut self, budget: usize) -> Self {
        self.budget = Some(budget);
        self
    }

    /// Set color.
    pub fn with_color(mut self, color: impl Into<String>) -> Self {
        self.color = Some(color.into());
        self
    }
}

/// Thinking level selector component.
///
/// For selecting reasoning/thinking effort levels (e.g., None, Low, Medium, High).
pub struct ThinkingSelector {
    levels: Vec<ThinkingLevel>,
    selected_id: Option<String>,
    select_list: SelectList,
}

impl ThinkingSelector {
    /// Create a new thinking selector with default levels.
    pub fn new() -> Self {
        let levels = vec![
            ThinkingLevel::new("none", "None", "No extended thinking")
                .with_color("gray"),
            ThinkingLevel::new("low", "Low", "Minimal reasoning effort")
                .with_budget(1024)
                .with_color("green"),
            ThinkingLevel::new("medium", "Medium", "Moderate reasoning effort")
                .with_budget(4096)
                .with_color("yellow"),
            ThinkingLevel::new("high", "High", "Maximum reasoning effort")
                .with_budget(16000)
                .with_color("red"),
        ];

        Self::with_levels(levels)
    }

    /// Create with custom levels.
    pub fn with_levels(levels: Vec<ThinkingLevel>) -> Self {
        let items: Vec<SelectItem> = levels
            .iter()
            .map(|l| {
                let desc = if let Some(budget) = l.budget {
                    format!("{} (~{} tokens)", l.description, budget)
                } else {
                    l.description.clone()
                };
                SelectItem::new(l.id.clone(), l.name.clone()).with_description(desc)
            })
            .collect();

        let theme = SelectListTheme {
            selected_prefix: Box::new(|s| {
                if s == "> " {
                    format!("\x1b[36m▶\x1b[0m ")
                } else {
                    format!("\x1b[36m{}\x1b[0m", s)
                }
            }),
            ..Default::default()
        };

        let mut select_list = SelectList::new(items, levels.len()).with_theme(theme);
        select_list.set_focused(true);

        Self {
            levels,
            selected_id: None,
            select_list,
        }
    }

    /// Get the selected level ID.
    pub fn selected_id(&self) -> Option<&str> {
        self.selected_id.as_deref()
    }

    /// Get the selected level.
    pub fn selected_level(&self) -> Option<&ThinkingLevel> {
        self.selected_id
            .as_ref()
            .and_then(|id| self.levels.iter().find(|l| l.id == *id))
    }

    /// Set selected by ID.
    pub fn set_selected(&mut self, id: &str) {
        self.selected_id = Some(id.to_string());
        // Find index and update select_list
        if let Some(idx) = self.levels.iter().position(|l| l.id == id) {
            // Would need set_selected_index on SelectList
        }
    }

    /// Confirm selection.
    pub fn confirm_selection(&mut self) {
        if let Some(item) = self.select_list.selected_item() {
            self.selected_id = Some(item.value.clone());
        }
    }

    /// Cycle to next level.
    pub fn next_level(&mut self) {
        self.select_list.handle_input("\x1b[B"); // Down arrow
        self.confirm_selection();
    }

    /// Cycle to previous level.
    pub fn prev_level(&mut self) {
        self.select_list.handle_input("\x1b[A"); // Up arrow
        self.confirm_selection();
    }

    /// Set on select callback.
    pub fn on_select<F>(&mut self, callback: F)
    where
        F: Fn(&ThinkingLevel) + Send + 'static,
    {
        let levels = self.levels.clone();
        self.select_list.on_select = Some(Box::new(move |item| {
            if let Some(level) = levels.iter().find(|l| l.id == item.value) {
                callback(level);
            }
        }));
    }
}

impl Default for ThinkingSelector {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for ThinkingSelector {
    fn render(&self, width: u16) -> Vec<String> {
        let mut lines = self.select_list.render(width);
        
        // Add header
        if !lines.is_empty() {
            let header = format!("\x1b[1mThinking Level\x1b[0m (Shift+Tab to cycle)");
            lines.insert(0, header);
        }
        
        lines
    }

    fn handle_input(&mut self, data: &str) -> InputResult {
        let kb = crate::keyboard::keybindings::KeybindingsManager::new();
        
        // Handle Shift+Tab to cycle
        if data == "\x1b[Z" {
            self.prev_level();
            return InputResult::Consumed;
        }
        
        // Handle Tab to cycle forward
        if kb.matches(data, EditorAction::Tab) {
            self.next_level();
            return InputResult::Consumed;
        }
        
        self.select_list.handle_input(data)
    }

    fn invalidate(&mut self) {
        self.select_list.invalidate();
    }

    fn is_dirty(&self) -> bool {
        self.select_list.is_dirty()
    }
}

impl Focusable for ThinkingSelector {
    fn focused(&self) -> bool {
        self.select_list.focused()
    }

    fn set_focused(&mut self, focused: bool) {
        self.select_list.set_focused(focused);
    }
}

/// Session selector for selecting from saved sessions.
pub struct SessionSelector {
    select_list: SelectList,
}

/// Quick action selector for slash commands.
pub struct QuickActionSelector {
    select_list: SelectList,
}

impl QuickActionSelector {
    /// Create a new quick action selector with common actions.
    pub fn new() -> Self {
        let actions = vec![
            ("/clear", "Clear conversation"),
            ("/compact", "Compact context"),
            ("/help", "Show help"),
            ("/model", "Change model"),
            ("/settings", "Open settings"),
            ("/tokens", "Show token usage"),
            ("/export", "Export conversation"),
            ("/quit", "Quit application"),
        ];

        let items: Vec<SelectItem> = actions
            .into_iter()
            .map(|(cmd, desc)| SelectItem::new(cmd.to_string(), cmd.to_string()).with_description(desc.to_string()))
            .collect();

        Self {
            select_list: SelectList::new(items, 8),
        }
    }
}

impl Default for QuickActionSelector {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for QuickActionSelector {
    fn render(&self, width: u16) -> Vec<String> {
        let mut lines = self.select_list.render(width);
        
        if !lines.is_empty() {
            lines.insert(0, "\x1b[1mCommands\x1b[0m".to_string());
        }
        
        lines
    }

    fn handle_input(&mut self, data: &str) -> InputResult {
        self.select_list.handle_input(data)
    }

    fn invalidate(&mut self) {
        self.select_list.invalidate();
    }

    fn is_dirty(&self) -> bool {
        self.select_list.is_dirty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_info() {
        let model = ModelInfo::new("claude-3-opus", "Claude 3 Opus", "Anthropic")
            .with_context_window(200_000)
            .with_costs(15.0, 75.0)
            .with_capabilities(vec!["vision".to_string(), "code".to_string()]);

        assert_eq!(model.display_label(), "Claude 3 Opus (Anthropic)");
        assert!(model.display_description().contains("200K"));
        assert!(model.display_description().contains("vision"));
    }

    #[test]
    fn test_thinking_level() {
        let level = ThinkingLevel::new("high", "High", "Maximum reasoning")
            .with_budget(16000)
            .with_color("red");

        assert_eq!(level.id, "high");
        assert_eq!(level.budget, Some(16000));
    }

    #[test]
    fn test_thinking_selector() {
        let selector = ThinkingSelector::new();
        assert!(!selector.levels.is_empty());
        assert_eq!(selector.selected_id(), None);
    }

    #[test]
    fn test_model_selector() {
        let models = vec![
            ModelInfo::new("gpt-4", "GPT-4", "OpenAI"),
            ModelInfo::new("claude-3", "Claude 3", "Anthropic"),
        ];

        let selector = ModelSelector::new(models);
        assert_eq!(selector.models.len(), 2);
    }
}
