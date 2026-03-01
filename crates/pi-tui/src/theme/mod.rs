//! Theme system with hot-reload support.
//!
//! Provides a unified theming system for all TUI components with support for:
//! - Built-in themes (default, dark, light, high-contrast)
//! - Custom theme files (JSON)
//! - Hot-reload of theme files
//! - Color palette management
//! - Semantic color naming

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// A color definition that can be specified in various formats.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Color {
    /// ANSI color name (e.g., "red", "bright-blue")
    Name(String),
    /// 8-bit color index (0-255)
    Indexed(u8),
    /// 24-bit RGB color (hex string like "#ff0000")
    Hex(String),
}

impl Color {
    /// Convert to ANSI escape sequence for foreground color.
    pub fn to_fg_ansi(&self) -> String {
        match self {
            Color::Name(name) => format!("\x1b[{}m", ansi_name_to_code(name, false)),
            Color::Indexed(n) => format!("\x1b[38;5;{}m", n),
            Color::Hex(hex) => {
                if let Some((r, g, b)) = parse_hex(hex) {
                    format!("\x1b[38;2;{};{};{}m", r, g, b)
                } else {
                    String::new()
                }
            }
        }
    }

    /// Convert to ANSI escape sequence for background color.
    pub fn to_bg_ansi(&self) -> String {
        match self {
            Color::Name(name) => format!("\x1b[{}m", ansi_name_to_code(name, true)),
            Color::Indexed(n) => format!("\x1b[48;5;{}m", n),
            Color::Hex(hex) => {
                if let Some((r, g, b)) = parse_hex(hex) {
                    format!("\x1b[48;2;{};{};{}m", r, g, b)
                } else {
                    String::new()
                }
            }
        }
    }
}

fn ansi_name_to_code(name: &str, bg: bool) -> u8 {
    let base = if bg { 40 } else { 30 };
    let bright_base = if bg { 100 } else { 90 };

    match name.to_lowercase().as_str() {
        "black" => base,
        "red" => base + 1,
        "green" => base + 2,
        "yellow" => base + 3,
        "blue" => base + 4,
        "magenta" => base + 5,
        "cyan" => base + 6,
        "white" => base + 7,
        "bright-black" | "gray" => bright_base,
        "bright-red" => bright_base + 1,
        "bright-green" => bright_base + 2,
        "bright-yellow" => bright_base + 3,
        "bright-blue" => bright_base + 4,
        "bright-magenta" => bright_base + 5,
        "bright-cyan" => bright_base + 6,
        "bright-white" => bright_base + 7,
        "default" => 39,
        _ => base + 7, // Default to white
    }
}

fn parse_hex(hex: &str) -> Option<(u8, u8, u8)> {
    let hex = hex.trim_start_matches('#');
    if hex.len() == 6 {
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        Some((r, g, b))
    } else {
        None
    }
}

/// Text styling attributes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Style {
    /// Foreground color
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fg: Option<Color>,
    /// Background color
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bg: Option<Color>,
    /// Bold text
    #[serde(default)]
    pub bold: bool,
    /// Italic text
    #[serde(default)]
    pub italic: bool,
    /// Underlined text
    #[serde(default)]
    pub underline: bool,
    /// Strikethrough text
    #[serde(default)]
    pub strikethrough: bool,
    /// Blinking text
    #[serde(default)]
    pub blink: bool,
}

impl Style {
    /// Create an empty style.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set foreground color.
    pub fn fg(mut self, color: Color) -> Self {
        self.fg = Some(color);
        self
    }

    /// Set background color.
    pub fn bg(mut self, color: Color) -> Self {
        self.bg = Some(color);
        self
    }

    /// Set bold.
    pub fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    /// Set italic.
    pub fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    /// Apply this style to a string.
    pub fn apply(&self, text: &str) -> String {
        let mut codes = Vec::new();

        if self.bold {
            codes.push("1");
        }
        if self.italic {
            codes.push("3");
        }
        if self.underline {
            codes.push("4");
        }
        if self.blink {
            codes.push("5");
        }
        if self.strikethrough {
            codes.push("9");
        }

        let mut result = String::new();

        if !codes.is_empty() {
            result.push_str(&format!("\x1b[{}m", codes.join(";")));
        }

        if let Some(ref fg) = self.fg {
            result.push_str(&fg.to_fg_ansi());
        }

        if let Some(ref bg) = self.bg {
            result.push_str(&bg.to_bg_ansi());
        }

        result.push_str(text);
        result.push_str("\x1b[0m");

        result
    }
}

/// Complete theme definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Theme {
    /// Theme name
    pub name: String,
    /// Semantic background colors
    pub bg: HashMap<String, Color>,
    /// Semantic foreground colors
    pub fg: HashMap<String, Color>,
    /// Component-specific styles
    pub components: HashMap<String, ComponentTheme>,
    /// Syntax highlighting colors
    pub syntax: HashMap<String, Color>,
}

/// Theme for a specific component.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ComponentTheme {
    /// Base style
    #[serde(flatten)]
    pub base: HashMap<String, serde_json::Value>,
}

impl Theme {
    /// Create a new theme with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            bg: HashMap::new(),
            fg: HashMap::new(),
            components: HashMap::new(),
            syntax: HashMap::new(),
        }
    }

    /// Get a foreground color by name.
    pub fn fg(&self, name: &str) -> Option<&Color> {
        self.fg.get(name)
    }

    /// Get a background color by name.
    pub fn bg(&self, name: &str) -> Option<&Color> {
        self.bg.get(name)
    }

    /// Get a component theme.
    pub fn component(&self, name: &str) -> Option<&ComponentTheme> {
        self.components.get(name)
    }

    /// Load a theme from a JSON file.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ThemeError> {
        let content = std::fs::read_to_string(path)?;
        let theme: Theme = serde_json::from_str(&content)?;
        Ok(theme)
    }

    /// Save theme to a JSON file.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), ThemeError> {
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }
}

/// Theme errors.
#[derive(Debug)]
pub enum ThemeError {
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl From<std::io::Error> for ThemeError {
    fn from(e: std::io::Error) -> Self {
        ThemeError::Io(e)
    }
}

impl From<serde_json::Error> for ThemeError {
    fn from(e: serde_json::Error) -> Self {
        ThemeError::Json(e)
    }
}

impl std::fmt::Display for ThemeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ThemeError::Io(e) => write!(f, "IO error: {}", e),
            ThemeError::Json(e) => write!(f, "JSON error: {}", e),
        }
    }
}

impl std::error::Error for ThemeError {}

/// Default dark theme.
pub fn dark_theme() -> Theme {
    let mut theme = Theme::new("dark");

    // Background colors
    theme
        .bg
        .insert("default".to_string(), Color::Name("default".to_string()));
    theme.bg.insert("panel".to_string(), Color::Indexed(236));
    theme.bg.insert("selected".to_string(), Color::Indexed(24));
    theme
        .bg
        .insert("highlight".to_string(), Color::Indexed(237));

    // Foreground colors
    theme
        .fg
        .insert("default".to_string(), Color::Name("white".to_string()));
    theme
        .fg
        .insert("muted".to_string(), Color::Name("bright-black".to_string()));
    theme
        .fg
        .insert("accent".to_string(), Color::Name("cyan".to_string()));
    theme
        .fg
        .insert("success".to_string(), Color::Name("green".to_string()));
    theme
        .fg
        .insert("warning".to_string(), Color::Name("yellow".to_string()));
    theme
        .fg
        .insert("error".to_string(), Color::Name("red".to_string()));
    theme
        .fg
        .insert("info".to_string(), Color::Name("blue".to_string()));

    // Syntax highlighting
    theme
        .syntax
        .insert("keyword".to_string(), Color::Name("magenta".to_string()));
    theme
        .syntax
        .insert("string".to_string(), Color::Name("green".to_string()));
    theme.syntax.insert(
        "comment".to_string(),
        Color::Name("bright-black".to_string()),
    );
    theme
        .syntax
        .insert("function".to_string(), Color::Name("blue".to_string()));
    theme
        .syntax
        .insert("variable".to_string(), Color::Name("cyan".to_string()));
    theme
        .syntax
        .insert("number".to_string(), Color::Name("yellow".to_string()));

    theme
}

/// Light theme.
pub fn light_theme() -> Theme {
    let mut theme = Theme::new("light");

    theme
        .fg
        .insert("default".to_string(), Color::Name("black".to_string()));
    theme.fg.insert("muted".to_string(), Color::Indexed(240));
    theme
        .fg
        .insert("accent".to_string(), Color::Name("blue".to_string()));
    theme
        .fg
        .insert("success".to_string(), Color::Name("green".to_string()));
    theme
        .fg
        .insert("warning".to_string(), Color::Name("yellow".to_string()));
    theme
        .fg
        .insert("error".to_string(), Color::Name("red".to_string()));

    theme
}

/// High contrast theme for accessibility.
pub fn high_contrast_theme() -> Theme {
    let mut theme = Theme::new("high-contrast");

    theme.fg.insert(
        "default".to_string(),
        Color::Name("bright-white".to_string()),
    );
    theme
        .bg
        .insert("default".to_string(), Color::Name("black".to_string()));
    theme
        .fg
        .insert("accent".to_string(), Color::Name("bright-cyan".to_string()));
    theme.fg.insert(
        "success".to_string(),
        Color::Name("bright-green".to_string()),
    );
    theme.fg.insert(
        "warning".to_string(),
        Color::Name("bright-yellow".to_string()),
    );
    theme
        .fg
        .insert("error".to_string(), Color::Name("bright-red".to_string()));

    theme
}

/// Theme manager with optional hot-reload support.
pub struct ThemeManager {
    current: Arc<RwLock<Theme>>,
    themes: HashMap<String, Theme>,
    watch_path: Option<PathBuf>,
    last_modified: Option<std::time::SystemTime>,
}

impl ThemeManager {
    /// Create a new theme manager with the default dark theme.
    pub fn new() -> Self {
        let mut themes = HashMap::new();
        themes.insert("dark".to_string(), dark_theme());
        themes.insert("light".to_string(), light_theme());
        themes.insert("high-contrast".to_string(), high_contrast_theme());

        Self {
            current: Arc::new(RwLock::new(dark_theme())),
            themes,
            watch_path: None,
            last_modified: None,
        }
    }

    /// Load a theme by name.
    pub fn load(&self, name: &str) -> Option<Theme> {
        self.themes.get(name).cloned()
    }

    /// Set the current theme by name.
    pub fn set_theme(&mut self, name: &str) -> Result<(), ThemeError> {
        if let Some(theme) = self.themes.get(name).cloned() {
            *self.current.write().unwrap() = theme;
            Ok(())
        } else {
            Err(ThemeError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Theme '{}' not found", name),
            )))
        }
    }

    /// Set theme from a file path with hot-reload support.
    pub fn set_theme_file(&mut self, path: impl AsRef<Path>) -> Result<(), ThemeError> {
        let path = path.as_ref().to_path_buf();
        let theme = Theme::from_file(&path)?;

        self.last_modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();

        self.watch_path = Some(path);
        *self.current.write().unwrap() = theme;

        Ok(())
    }

    /// Get the current theme.
    pub fn current(&self) -> Theme {
        self.current.read().unwrap().clone()
    }

    /// Check if theme file has changed and reload if necessary.
    pub fn check_reload(&mut self) -> Result<bool, ThemeError> {
        if let Some(ref path) = self.watch_path {
            let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();

            if modified != self.last_modified {
                if let Some(new_theme) = Theme::from_file(path).ok() {
                    *self.current.write().unwrap() = new_theme;
                    self.last_modified = modified;
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// Register a custom theme.
    pub fn register(&mut self, name: impl Into<String>, theme: Theme) {
        self.themes.insert(name.into(), theme);
    }

    /// List available theme names.
    pub fn list_themes(&self) -> Vec<&String> {
        self.themes.keys().collect()
    }
}

impl Default for ThemeManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_color_to_ansi() {
        let red = Color::Name("red".to_string());
        assert_eq!(red.to_fg_ansi(), "\x1b[31m");

        let indexed = Color::Indexed(100);
        assert_eq!(indexed.to_fg_ansi(), "\x1b[38;5;100m");

        let hex = Color::Hex("#ff0000".to_string());
        assert_eq!(hex.to_fg_ansi(), "\x1b[38;2;255;0;0m");
    }

    #[test]
    fn test_style_apply() {
        let style = Style::new().fg(Color::Name("red".to_string())).bold();

        let styled = style.apply("Hello");
        assert!(styled.contains("\x1b["));
        assert!(styled.contains("Hello"));
        assert!(styled.contains("\x1b[0m"));
    }

    #[test]
    fn test_dark_theme() {
        let theme = dark_theme();
        assert_eq!(theme.name, "dark");
        assert!(theme.fg.contains_key("default"));
        assert!(theme.syntax.contains_key("keyword"));
    }

    #[test]
    fn test_theme_manager() {
        let mut manager = ThemeManager::new();

        assert!(manager.load("dark").is_some());
        assert!(manager.load("nonexistent").is_none());

        manager.set_theme("dark").unwrap();
        let current = manager.current();
        assert_eq!(current.name, "dark");
    }

    #[test]
    fn test_parse_hex() {
        assert_eq!(parse_hex("#ff0000"), Some((255, 0, 0)));
        assert_eq!(parse_hex("#00ff00"), Some((0, 255, 0)));
        assert_eq!(parse_hex("#0000ff"), Some((0, 0, 255)));
        assert_eq!(parse_hex("invalid"), None);
    }
}
