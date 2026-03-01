//! Slash command parser for TUI interactions.
//!
//! Provides parsing and handling of slash commands like:
//! - `/clear` - Clear conversation
//! - `/model <name>` - Switch model
//! - `/thinking <level>` - Set thinking level
//! - `/help` - Show help
//! - And more...

use std::collections::HashMap;

/// A parsed slash command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashCommand {
    /// Command name (without the leading slash)
    pub name: String,
    /// Command arguments
    pub args: Vec<String>,
    /// Full original text
    pub raw: String,
}

impl SlashCommand {
    /// Parse a command from input text.
    pub fn parse(input: &str) -> Option<Self> {
        let trimmed = input.trim();
        
        if !trimmed.starts_with('/') {
            return None;
        }

        let parts: Vec<&str> = trimmed[1..].split_whitespace().collect();
        if parts.is_empty() {
            return None;
        }

        let name = parts[0].to_string();
        let args = parts[1..].iter().map(|s| s.to_string()).collect();

        Some(Self {
            name,
            args,
            raw: trimmed.to_string(),
        })
    }

    /// Get the first argument.
    pub fn first_arg(&self) -> Option<&str> {
        self.args.first().map(|s| s.as_str())
    }

    /// Check if the command has no arguments.
    pub fn has_no_args(&self) -> bool {
        self.args.is_empty()
    }

    /// Check if this matches a command name (case-insensitive).
    pub fn is(&self, name: &str) -> bool {
        self.name.eq_ignore_ascii_case(name)
    }
}

/// Definition of a slash command.
#[derive(Debug, Clone)]
pub struct CommandDef {
    /// Command name
    pub name: String,
    /// Short description
    pub description: String,
    /// Usage syntax
    pub usage: String,
    /// Whether arguments are required
    pub args_required: bool,
    /// Argument descriptions
    pub arg_help: Vec<(String, String)>,
    /// Aliases for this command
    pub aliases: Vec<String>,
}

impl CommandDef {
    /// Create a new command definition.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            usage: String::new(),
            args_required: false,
            arg_help: Vec::new(),
            aliases: Vec::new(),
        }
    }

    /// Set description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    /// Set usage syntax.
    pub fn with_usage(mut self, usage: impl Into<String>) -> Self {
        self.usage = usage.into();
        self
    }

    /// Set args required.
    pub fn with_args_required(mut self, required: bool) -> Self {
        self.args_required = required;
        self
    }

    /// Add argument help.
    pub fn with_arg(mut self, name: impl Into<String>, desc: impl Into<String>) -> Self {
        self.arg_help.push((name.into(), desc.into()));
        self
    }

    /// Add alias.
    pub fn with_alias(mut self, alias: impl Into<String>) -> Self {
        self.aliases.push(alias.into());
        self
    }

    /// Check if name matches this command or any alias.
    pub fn matches(&self, name: &str) -> bool {
        self.name.eq_ignore_ascii_case(name)
            || self.aliases.iter().any(|a| a.eq_ignore_ascii_case(name))
    }
}

/// Registry of slash commands.
pub struct SlashCommandRegistry {
    commands: HashMap<String, CommandDef>,
}

impl SlashCommandRegistry {
    /// Create a new registry with built-in commands.
    pub fn new() -> Self {
        let mut registry = Self {
            commands: HashMap::new(),
        };
        registry.register_defaults();
        registry
    }

    /// Register a command definition.
    pub fn register(&mut self, def: CommandDef) {
        self.commands.insert(def.name.to_lowercase(), def);
    }

    /// Get a command definition by name.
    pub fn get(&self, name: &str) -> Option<&CommandDef> {
        self.commands.get(&name.to_lowercase())
    }

    /// Check if a command exists.
    pub fn has(&self, name: &str) -> bool {
        self.commands.contains_key(&name.to_lowercase())
    }

    /// List all registered commands.
    pub fn list(&self) -> Vec<&CommandDef> {
        self.commands.values().collect()
    }

    /// Get commands grouped by category.
    pub fn by_category(&self) -> HashMap<&str, Vec<&CommandDef>> {
        let mut groups: HashMap<&str, Vec<&CommandDef>> = HashMap::new();
        
        for cmd in self.commands.values() {
            let category = match cmd.name.as_str() {
                "clear" | "compact" | "undo" => "Session",
                "model" | "thinking" | "temperature" => "Model",
                "settings" | "config" => "Settings",
                "help" | "commands" => "Help",
                _ => "Other",
            };
            groups.entry(category).or_default().push(cmd);
        }
        
        groups
    }

    fn register_defaults(&mut self) {
        // Session commands
        self.register(
            CommandDef::new("clear", "Clear the conversation history")
                .with_alias("cls"),
        );

        self.register(
            CommandDef::new("compact", "Compact the conversation context")
                .with_description("Summarize and truncate conversation to save tokens"),
        );

        self.register(
            CommandDef::new("undo", "Undo the last message")
                .with_alias("back"),
        );

        // Model commands
        self.register(
            CommandDef::new("model", "Switch to a different AI model")
                .with_usage("/model <name>")
                .with_arg("name", "Model name (e.g., gpt-4, claude-3-opus)")
                .with_args_required(true),
        );

        self.register(
            CommandDef::new("models", "List available models")
                .with_alias("list-models"),
        );

        self.register(
            CommandDef::new("thinking", "Set the thinking level")
                .with_usage("/thinking <none|low|medium|high>")
                .with_arg("level", "Thinking level: none, low, medium, or high"),
        );

        // Settings
        self.register(
            CommandDef::new("settings", "Open settings")
                .with_alias("config")
                .with_alias("prefs"),
        );

        self.register(
            CommandDef::new("tokens", "Show token usage statistics")
                .with_alias("usage"),
        );

        // Export/Import
        self.register(
            CommandDef::new("export", "Export conversation to file")
                .with_usage("/export [format]")
                .with_arg("format", "Export format: json, md, html (default: md)"),
        );

        self.register(
            CommandDef::new("save", "Save conversation to a named session")
                .with_usage("/save <name>")
                .with_arg("name", "Session name")
                .with_args_required(true),
        );

        self.register(
            CommandDef::new("load", "Load a saved session")
                .with_usage("/load <name>")
                .with_arg("name", "Session name")
                .with_args_required(true),
        );

        // Help
        self.register(
            CommandDef::new("help", "Show help information")
                .with_usage("/help [command]")
                .with_arg("command", "Specific command to get help for"),
        );

        self.register(
            CommandDef::new("commands", "List all available commands")
                .with_alias("cmds"),
        );

        // System
        self.register(
            CommandDef::new("quit", "Quit the application")
                .with_alias("exit")
                .with_alias("q"),
        );

        self.register(
            CommandDef::new("version", "Show version information")
                .with_alias("v"),
        );

        // Additional commands (new)
        self.register(
            CommandDef::new("history", "Show conversation history")
                .with_alias("hist")
                .with_usage("/history [limit]")
                .with_arg("limit", "Number of messages to show (default: 10)"),
        );

        self.register(
            CommandDef::new("fork", "Fork the current session")
                .with_description("Create a new session branching from current point")
                .with_usage("/fork [name]")
                .with_arg("name", "Optional name for the forked session"),
        );

        self.register(
            CommandDef::new("merge", "Merge another session into current")
                .with_usage("/merge <session-name>")
                .with_arg("session-name", "Name of session to merge")
                .with_args_required(true),
        );

        self.register(
            CommandDef::new("debug", "Toggle debug mode")
                .with_description("Show detailed tool execution and timing info"),
        );

        self.register(
            CommandDef::new("prompt", "View or edit system prompt")
                .with_usage("/prompt [new-prompt]")
                .with_arg("new-prompt", "New system prompt (omit to view current)"),
        );
    }
}

impl Default for SlashCommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of executing a slash command.
#[derive(Debug, Clone)]
pub enum CommandResult {
    /// Command executed successfully
    Success(Option<String>),
    /// Command failed with error message
    Error(String),
    /// Command requires confirmation
    Confirm { prompt: String, command: SlashCommand },
    /// Command showed help/info
    Info(String),
}

/// Handler for slash commands.
pub trait CommandHandler {
    /// Handle a parsed command and return the result.
    fn handle(&mut self, cmd: &SlashCommand) -> CommandResult;
}

/// Simple in-memory command handler for testing.
pub struct SimpleCommandHandler {
    registry: SlashCommandRegistry,
}

impl SimpleCommandHandler {
    /// Create a new simple handler.
    pub fn new() -> Self {
        Self {
            registry: SlashCommandRegistry::new(),
        }
    }

    /// Get help text for all commands.
    pub fn help_all(&self) -> String {
        let mut result = String::from("Available commands:\n\n");
        
        for category in ["Session", "Model", "Settings", "Export", "Help", "Debug", "Other"] {
            let commands: Vec<&CommandDef> = self.registry.list()
                .into_iter()
                .filter(|c| {
                    let cat = match c.name.as_str() {
                        "clear" | "compact" | "undo" | "fork" | "merge" => "Session",
                        "model" | "thinking" | "models" => "Model",
                        "settings" | "tokens" | "prompt" => "Settings",
                        "export" | "save" | "load" => "Export",
                        "help" | "commands" | "version" | "history" => "Help",
                        "debug" => "Debug",
                        _ => "Other",
                    };
                    cat == category
                })
                .collect();
            
            if !commands.is_empty() {
                result.push_str(&format!("\x1b[1m{}:\x1b[0m\n", category));
                for cmd in commands {
                    result.push_str(&format!("  /{:<15} {}\n", cmd.name, cmd.description));
                }
                result.push('\n');
            }
        }
        
        result
    }

    /// Get help for a specific command.
    pub fn help_command(&self, name: &str) -> String {
        if let Some(cmd) = self.registry.get(name) {
            let mut result = format!("\x1b[1m/{}{}\x1b[0m\n", cmd.name,
                if cmd.usage.is_empty() { String::new() } else { format!(" {}", cmd.usage) });
            result.push_str(&format!("\n{}\n", cmd.description));
            
            if !cmd.arg_help.is_empty() {
                result.push_str("\nArguments:\n");
                for (arg, desc) in &cmd.arg_help {
                    result.push_str(&format!("  <{:<12}> {}\n", arg, desc));
                }
            }
            
            if !cmd.aliases.is_empty() {
                result.push_str(&format!("\nAliases: {}\n", cmd.aliases.join(", ")));
            }
            
            result
        } else {
            format!("Unknown command: /{}\nType /help for a list of commands.", name)
        }
    }
}

impl Default for SimpleCommandHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandHandler for SimpleCommandHandler {
    fn handle(&mut self, cmd: &SlashCommand) -> CommandResult {
        match cmd.name.as_str() {
            "help" | "h" => {
                if let Some(arg) = cmd.first_arg() {
                    CommandResult::Info(self.help_command(arg))
                } else {
                    CommandResult::Info(self.help_all())
                }
            }
            "commands" | "cmds" => {
                CommandResult::Info(self.help_all())
            }
            "clear" | "cls" => {
                CommandResult::Success(Some("Conversation cleared.".to_string()))
            }
            "quit" | "exit" | "q" => {
                CommandResult::Success(None)
            }
            "version" | "v" => {
                CommandResult::Info(format!("pi-tui version {}", env!("CARGO_PKG_VERSION")))
            }
            "history" | "hist" => {
                CommandResult::Success(Some("Showing conversation history...".to_string()))
            }
            "fork" => {
                CommandResult::Success(Some("Session forked.".to_string()))
            }
            "merge" => {
                CommandResult::Success(Some("Session merged.".to_string()))
            }
            "debug" => {
                CommandResult::Success(Some("Debug mode toggled.".to_string()))
            }
            "prompt" => {
                CommandResult::Success(Some("System prompt updated.".to_string()))
            }
            _ => {
                if self.registry.has(&cmd.name) {
                    CommandResult::Success(Some(format!("Command '{}' executed", cmd.name)))
                } else {
                    CommandResult::Error(format!("Unknown command: /{}\nType /help for a list of commands.", cmd.name))
                }
            }
        }
    }
}

/// Check if input is a slash command.
pub fn is_slash_command(input: &str) -> bool {
    SlashCommand::parse(input).is_some()
}

/// Get command completions for partial input.
pub fn complete_command(partial: &str, registry: &SlashCommandRegistry) -> Vec<String> {
    if !partial.starts_with('/') {
        return Vec::new();
    }

    let name = &partial[1..].to_lowercase();
    
    registry.list()
        .into_iter()
        .filter(|cmd| cmd.name.starts_with(name))
        .map(|cmd| format!("/{}", cmd.name))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_command() {
        let cmd = SlashCommand::parse("/clear").unwrap();
        assert_eq!(cmd.name, "clear");
        assert!(cmd.has_no_args());

        let cmd = SlashCommand::parse("/model gpt-4").unwrap();
        assert_eq!(cmd.name, "model");
        assert_eq!(cmd.args, vec!["gpt-4"]);

        let cmd = SlashCommand::parse("/help model").unwrap();
        assert_eq!(cmd.name, "help");
        assert_eq!(cmd.first_arg(), Some("model"));
    }

    #[test]
    fn test_not_command() {
        assert!(SlashCommand::parse("hello world").is_none());
        assert!(SlashCommand::parse("  /clear  ").is_some());
    }

    #[test]
    fn test_command_def() {
        let def = CommandDef::new("test", "Test command")
            .with_usage("/test <arg>")
            .with_arg("arg", "An argument")
            .with_alias("t");

        assert!(def.matches("test"));
        assert!(def.matches("TEST"));
        assert!(def.matches("t"));
    }

    #[test]
    fn test_registry() {
        let registry = SlashCommandRegistry::new();
        
        assert!(registry.has("help"));
        assert!(registry.has("clear"));
        assert!(registry.has("model"));
        assert!(!registry.has("nonexistent"));

        let help = registry.get("help").unwrap();
        assert_eq!(help.name, "help");
    }

    #[test]
    fn test_handler() {
        let mut handler = SimpleCommandHandler::new();
        
        let cmd = SlashCommand::parse("/help").unwrap();
        let result = handler.handle(&cmd);
        
        match result {
            CommandResult::Info(text) => {
                assert!(text.contains("Available commands"));
            }
            _ => panic!("Expected Info result"),
        }
    }

    #[test]
    fn test_complete_command() {
        let registry = SlashCommandRegistry::new();
        
        let completions = complete_command("/he", &registry);
        assert!(completions.iter().any(|c| c == "/help"));
        
        let completions = complete_command("/mod", &registry);
        assert!(completions.iter().any(|c| c == "/model"));
    }
}
