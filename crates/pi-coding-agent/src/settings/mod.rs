use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Top-level agent settings. Every field is optional so that partial config
/// files are valid — missing keys simply fall through to the defaults.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Settings {
    /// Provider id, e.g. "anthropic", "openai".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<String>,

    /// Model name, e.g. "claude-opus-4-6".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,

    /// Thinking level: "none" | "low" | "medium" | "high".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<String>,

    /// Context compaction configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compaction: Option<CompactionConfig>,

    /// Shell used for executing commands, e.g. "/bin/bash".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,

    /// UI theme name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,

    /// Maximum agentic turns before the agent stops.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<usize>,
}

/// Compaction sub-config nested inside [`Settings`].
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct CompactionConfig {
    /// Whether automatic compaction is active.
    pub enabled: Option<bool>,

    /// Token budget reserved for the compaction summary.
    pub reserve_tokens: Option<u64>,
}

// ---------------------------------------------------------------------------
// SettingsManager
// ---------------------------------------------------------------------------

/// Resolves and manages the two-level settings hierarchy:
///
/// * **User settings** — `~/.pi/agent/settings.json`
/// * **Project settings** — `<cwd>/.pi/settings.json`
///
/// Project settings overlay user settings; individual `Some` fields in the
/// project file win over matching fields in the user file.
pub struct SettingsManager {
    /// `~/.pi/agent/settings.json`
    user_settings_path: PathBuf,
    /// `<cwd>/.pi/settings.json`
    project_settings_path: PathBuf,
}

impl SettingsManager {
    /// Create a new manager resolving paths from the real environment.
    ///
    /// * `cwd` — the working directory to look for `.pi/settings.json` in.
    ///
    /// The user-level path is resolved from `$HOME` (or `/tmp` as fallback).
    pub fn new(cwd: &Path) -> Self {
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp"));

        Self::with_paths(
            home.join(".pi").join("agent").join("settings.json"),
            cwd.join(".pi").join("settings.json"),
        )
    }

    /// Create a manager with explicit paths. Useful for testing.
    pub fn with_paths(user_settings_path: PathBuf, project_settings_path: PathBuf) -> Self {
        Self {
            user_settings_path,
            project_settings_path,
        }
    }

    /// Load the effective [`Settings`] by deep-merging user → project.
    ///
    /// Missing or malformed files are silently treated as empty settings
    /// rather than hard errors, so the agent always gets a usable value.
    pub fn load(&self) -> Settings {
        let user = self.load_file(&self.user_settings_path);
        let project = self.load_file(&self.project_settings_path);

        let mut merged = user;
        deep_merge(&mut merged, &project);
        merged
    }

    /// Persist `settings` to the user-level path.
    ///
    /// Parent directories are created automatically.
    pub fn save_user(&self, settings: &Settings) -> anyhow::Result<()> {
        self.write_file(&self.user_settings_path, settings)
    }

    /// Persist `settings` to the project-level path.
    ///
    /// Parent directories are created automatically.
    pub fn save_project(&self, settings: &Settings) -> anyhow::Result<()> {
        self.write_file(&self.project_settings_path, settings)
    }

    // ------------------------------------------------------------------
    // helpers
    // ------------------------------------------------------------------

    /// Load a single settings file. Returns [`Settings::default()`] on any
    /// error (file not found, parse error, etc.).
    fn load_file(&self, path: &Path) -> Settings {
        match fs::read_to_string(path) {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
            Err(_) => Settings::default(),
        }
    }

    /// Serialise `settings` as pretty-printed JSON and write to `path`,
    /// creating parent directories as required.
    fn write_file(&self, path: &Path, settings: &Settings) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(settings)?;
        fs::write(path, json)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Deep merge
// ---------------------------------------------------------------------------

/// Merge `overlay` into `base` field-by-field.
///
/// A `Some` value in `overlay` replaces the corresponding field in `base`.
/// `None` values in `overlay` leave `base` fields untouched.
pub fn deep_merge(base: &mut Settings, overlay: &Settings) {
    if overlay.default_provider.is_some() {
        base.default_provider = overlay.default_provider.clone();
    }
    if overlay.default_model.is_some() {
        base.default_model = overlay.default_model.clone();
    }
    if overlay.thinking_level.is_some() {
        base.thinking_level = overlay.thinking_level.clone();
    }
    if overlay.shell.is_some() {
        base.shell = overlay.shell.clone();
    }
    if overlay.theme.is_some() {
        base.theme = overlay.theme.clone();
    }
    if overlay.max_turns.is_some() {
        base.max_turns = overlay.max_turns;
    }

    // Compaction is merged recursively so that a project file can override
    // only `reserve_tokens` without having to repeat `enabled`.
    match (&mut base.compaction, &overlay.compaction) {
        (_, None) => {
            // overlay has no compaction config — nothing to do
        }
        (None, Some(ov)) => {
            base.compaction = Some(ov.clone());
        }
        (Some(ref mut base_c), Some(ref ov_c)) => {
            deep_merge_compaction(base_c, ov_c);
        }
    }
}

/// Deep merge for [`CompactionConfig`].
fn deep_merge_compaction(base: &mut CompactionConfig, overlay: &CompactionConfig) {
    if overlay.enabled.is_some() {
        base.enabled = overlay.enabled;
    }
    if overlay.reserve_tokens.is_some() {
        base.reserve_tokens = overlay.reserve_tokens;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // -----------------------------------------------------------------------
    // Helper: build a SettingsManager whose user dir AND project dir both
    // point into a private temporary directory, avoiding any env-var mutation
    // so tests are fully isolated from each other even when run in parallel.
    // -----------------------------------------------------------------------
    fn make_manager(tmp: &TempDir) -> SettingsManager {
        let user_settings_path = tmp
            .path()
            .join("home")
            .join(".pi")
            .join("agent")
            .join("settings.json");

        let project_settings_path = tmp
            .path()
            .join("project")
            .join(".pi")
            .join("settings.json");

        SettingsManager::with_paths(user_settings_path, project_settings_path)
    }

    // -----------------------------------------------------------------------
    // Test 1 — load returns defaults when no files exist
    // -----------------------------------------------------------------------
    #[test]
    fn test_load_returns_defaults_when_no_files() {
        let tmp = TempDir::new().unwrap();
        let mgr = make_manager(&tmp);

        let settings = mgr.load();

        assert_eq!(settings, Settings::default());
        assert!(settings.default_provider.is_none());
        assert!(settings.default_model.is_none());
        assert!(settings.max_turns.is_none());
    }

    // -----------------------------------------------------------------------
    // Test 2 — project settings overlay wins over user settings
    // -----------------------------------------------------------------------
    #[test]
    fn test_project_overlay_wins_over_user() {
        let tmp = TempDir::new().unwrap();
        let mgr = make_manager(&tmp);

        // Write user settings
        let user = Settings {
            default_provider: Some("anthropic".to_string()),
            default_model: Some("claude-opus-4-6".to_string()),
            max_turns: Some(10),
            ..Default::default()
        };
        mgr.save_user(&user).unwrap();

        // Write project settings that override model and max_turns
        let project = Settings {
            default_model: Some("claude-sonnet-4-6".to_string()),
            max_turns: Some(5),
            ..Default::default()
        };
        mgr.save_project(&project).unwrap();

        let effective = mgr.load();

        // provider comes from user (project didn't set it)
        assert_eq!(effective.default_provider.as_deref(), Some("anthropic"));
        // model and max_turns come from project (overlay wins)
        assert_eq!(effective.default_model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(effective.max_turns, Some(5));
    }

    // -----------------------------------------------------------------------
    // Test 3 — save / load round-trip preserves all fields
    // -----------------------------------------------------------------------
    #[test]
    fn test_save_load_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let mgr = make_manager(&tmp);

        let original = Settings {
            default_provider: Some("openai".to_string()),
            default_model: Some("gpt-4o".to_string()),
            thinking_level: Some("high".to_string()),
            shell: Some("/bin/zsh".to_string()),
            theme: Some("dark".to_string()),
            max_turns: Some(20),
            compaction: Some(CompactionConfig {
                enabled: Some(true),
                reserve_tokens: Some(4096),
            }),
        };

        mgr.save_user(&original).unwrap();

        // Load with no project file present — should equal original exactly.
        let loaded = mgr.load();

        assert_eq!(loaded.default_provider.as_deref(), Some("openai"));
        assert_eq!(loaded.default_model.as_deref(), Some("gpt-4o"));
        assert_eq!(loaded.thinking_level.as_deref(), Some("high"));
        assert_eq!(loaded.shell.as_deref(), Some("/bin/zsh"));
        assert_eq!(loaded.theme.as_deref(), Some("dark"));
        assert_eq!(loaded.max_turns, Some(20));

        let compaction = loaded.compaction.as_ref().unwrap();
        assert_eq!(compaction.enabled, Some(true));
        assert_eq!(compaction.reserve_tokens, Some(4096));
    }

    // -----------------------------------------------------------------------
    // Test 4 — empty overlay leaves base completely unchanged
    // -----------------------------------------------------------------------
    #[test]
    fn test_empty_overlay_is_noop() {
        let mut base = Settings {
            default_provider: Some("anthropic".to_string()),
            default_model: Some("claude-opus-4-6".to_string()),
            thinking_level: Some("medium".to_string()),
            max_turns: Some(15),
            compaction: Some(CompactionConfig {
                enabled: Some(false),
                reserve_tokens: Some(2048),
            }),
            ..Default::default()
        };

        let original = base.clone();
        let empty_overlay = Settings::default();

        deep_merge(&mut base, &empty_overlay);

        assert_eq!(base, original);
    }

    // -----------------------------------------------------------------------
    // Test 5 — compaction sub-fields are merged independently
    // -----------------------------------------------------------------------
    #[test]
    fn test_compaction_deep_merge() {
        let tmp = TempDir::new().unwrap();
        let mgr = make_manager(&tmp);

        let user = Settings {
            compaction: Some(CompactionConfig {
                enabled: Some(false),
                reserve_tokens: Some(1024),
            }),
            ..Default::default()
        };
        mgr.save_user(&user).unwrap();

        // Project overrides only reserve_tokens, not enabled
        let project = Settings {
            compaction: Some(CompactionConfig {
                enabled: None,
                reserve_tokens: Some(8192),
            }),
            ..Default::default()
        };
        mgr.save_project(&project).unwrap();

        let effective = mgr.load();
        let c = effective.compaction.unwrap();

        // enabled came from user
        assert_eq!(c.enabled, Some(false));
        // reserve_tokens came from project
        assert_eq!(c.reserve_tokens, Some(8192));
    }

    // -----------------------------------------------------------------------
    // Test 6 — malformed JSON in a settings file is silently ignored
    // -----------------------------------------------------------------------
    #[test]
    fn test_malformed_json_falls_back_to_default() {
        let tmp = TempDir::new().unwrap();
        let mgr = make_manager(&tmp);

        // Write garbage to the user settings path
        if let Some(parent) = mgr.user_settings_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&mgr.user_settings_path, b"{ NOT VALID JSON !!!").unwrap();

        let settings = mgr.load();
        assert_eq!(settings, Settings::default());
    }
}
