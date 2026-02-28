use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;

/// Context file names to search for, in priority order.
/// AGENTS.md is preferred over CLAUDE.md when both exist in the same directory.
const CONTEXT_FILENAMES: &[&str] = &["AGENTS.md", "CLAUDE.md"];

/// Loaded context files from the filesystem.
#[derive(Debug, Default)]
pub struct LoadedContext {
    /// AGENTS.md / CLAUDE.md files found (path, content) — ordered from global to local.
    pub agents_files: Vec<(PathBuf, String)>,
    /// System prompt from SYSTEM.md if found.
    pub system_prompt: Option<String>,
    /// Append system prompt from APPEND_SYSTEM.md if found.
    pub append_system_prompt: Option<String>,
}

/// Load all context files for the given working directory.
///
/// Search order for AGENTS.md / CLAUDE.md:
/// 1. Global: `~/.pi/AGENTS.md` or `~/.pi/CLAUDE.md`
/// 2. Walk from filesystem root down to `cwd`, collecting each directory's
///    AGENTS.md or CLAUDE.md (root-first so the order is global -> ancestor -> project).
///
/// Search order for SYSTEM.md:
/// 1. `{cwd}/.pi/SYSTEM.md`
/// 2. `~/.pi/SYSTEM.md`
///
/// Search order for APPEND_SYSTEM.md:
/// 1. `{cwd}/.pi/APPEND_SYSTEM.md`
/// 2. `~/.pi/APPEND_SYSTEM.md`
pub fn load_context(cwd: &Path) -> Result<LoadedContext> {
    let mut ctx = LoadedContext::default();
    let agent_dir = agent_config_dir();

    // -- 1. Load AGENTS.md / CLAUDE.md files -----------------------------------

    // Global config first
    let mut seen: HashSet<PathBuf> = HashSet::new();
    if let Some((path, content)) = load_context_file_from_dir(&agent_dir) {
        seen.insert(path.clone());
        ctx.agents_files.push((path, content));
    }

    // Walk ancestors from root to cwd (so ordering is global -> ancestor -> project)
    let mut ancestors: Vec<PathBuf> = Vec::new();
    let mut current = cwd.to_path_buf();
    loop {
        ancestors.push(current.clone());
        if !current.pop() {
            break;
        }
    }
    ancestors.reverse(); // root first

    for dir in &ancestors {
        if let Some((path, content)) = load_context_file_from_dir(dir) {
            if seen.insert(path.clone()) {
                ctx.agents_files.push((path, content));
            }
        }
    }

    // -- 2. Load SYSTEM.md -----------------------------------------------------

    let project_system = cwd.join(".pi").join("SYSTEM.md");
    let global_system = agent_dir.join("SYSTEM.md");
    if project_system.exists() {
        ctx.system_prompt = std::fs::read_to_string(&project_system).ok();
    } else if global_system.exists() {
        ctx.system_prompt = std::fs::read_to_string(&global_system).ok();
    }

    // -- 3. Load APPEND_SYSTEM.md ----------------------------------------------

    let project_append = cwd.join(".pi").join("APPEND_SYSTEM.md");
    let global_append = agent_dir.join("APPEND_SYSTEM.md");
    if project_append.exists() {
        ctx.append_system_prompt = std::fs::read_to_string(&project_append).ok();
    } else if global_append.exists() {
        ctx.append_system_prompt = std::fs::read_to_string(&global_append).ok();
    }

    Ok(ctx)
}

/// Build the full system prompt from loaded context and optional CLI override.
///
/// Priority:
/// 1. If `cli_system_prompt` is `Some`, use it as the base.
/// 2. Else if `SYSTEM.md` was found, use it as the base.
/// 3. Else use `default_prompt` as the base.
/// 4. Append all AGENTS.md / CLAUDE.md content.
/// 5. Append APPEND_SYSTEM.md if present.
pub fn build_system_prompt(
    loaded: &LoadedContext,
    cli_system_prompt: Option<&str>,
    default_prompt: &str,
) -> String {
    let mut parts: Vec<String> = Vec::new();

    // Base prompt
    let base = cli_system_prompt
        .or(loaded.system_prompt.as_deref())
        .unwrap_or(default_prompt);
    parts.push(base.to_string());

    // AGENTS.md / CLAUDE.md content
    for (path, content) in &loaded.agents_files {
        parts.push(format!(
            "\n# Context from {}\n{}",
            path.display(),
            content
        ));
    }

    // APPEND_SYSTEM.md
    if let Some(ref append) = loaded.append_system_prompt {
        parts.push(format!("\n{}", append));
    }

    parts.join("\n")
}

/// Return the global agent config directory (`~/.pi`).
fn agent_config_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".pi")
}

/// Try to load AGENTS.md or CLAUDE.md from the given directory.
/// Returns the first file found in priority order (AGENTS.md first).
fn load_context_file_from_dir(dir: &Path) -> Option<(PathBuf, String)> {
    for filename in CONTEXT_FILENAMES {
        let path = dir.join(filename);
        if path.exists() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                return Some((path, content));
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_load_context_file_from_dir_finds_agents_md() {
        let tmp = TempDir::new().unwrap();
        let agents_path = tmp.path().join("AGENTS.md");
        fs::write(&agents_path, "# Agents instructions").unwrap();

        let result = load_context_file_from_dir(tmp.path());
        assert!(result.is_some());
        let (path, content) = result.unwrap();
        assert_eq!(path, agents_path);
        assert_eq!(content, "# Agents instructions");
    }

    #[test]
    fn test_load_context_file_from_dir_finds_claude_md() {
        let tmp = TempDir::new().unwrap();
        let claude_path = tmp.path().join("CLAUDE.md");
        fs::write(&claude_path, "# Claude instructions").unwrap();

        let result = load_context_file_from_dir(tmp.path());
        assert!(result.is_some());
        let (path, content) = result.unwrap();
        assert_eq!(path, claude_path);
        assert_eq!(content, "# Claude instructions");
    }

    #[test]
    fn test_load_context_file_from_dir_prefers_agents_over_claude() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("AGENTS.md"), "agents content").unwrap();
        fs::write(tmp.path().join("CLAUDE.md"), "claude content").unwrap();

        let result = load_context_file_from_dir(tmp.path());
        assert!(result.is_some());
        let (path, content) = result.unwrap();
        assert_eq!(path, tmp.path().join("AGENTS.md"));
        assert_eq!(content, "agents content");
    }

    #[test]
    fn test_load_context_file_from_dir_returns_none_when_empty() {
        let tmp = TempDir::new().unwrap();
        let result = load_context_file_from_dir(tmp.path());
        assert!(result.is_none());
    }

    #[test]
    fn test_build_system_prompt_uses_default_when_no_context() {
        let ctx = LoadedContext::default();
        let prompt = build_system_prompt(&ctx, None, "default prompt");
        assert_eq!(prompt, "default prompt");
    }

    #[test]
    fn test_build_system_prompt_cli_overrides_everything() {
        let ctx = LoadedContext {
            system_prompt: Some("from SYSTEM.md".to_string()),
            ..Default::default()
        };
        let prompt = build_system_prompt(&ctx, Some("cli override"), "default");
        assert!(prompt.starts_with("cli override"));
        assert!(!prompt.contains("from SYSTEM.md"));
        assert!(!prompt.contains("default"));
    }

    #[test]
    fn test_build_system_prompt_system_md_overrides_default() {
        let ctx = LoadedContext {
            system_prompt: Some("from SYSTEM.md".to_string()),
            ..Default::default()
        };
        let prompt = build_system_prompt(&ctx, None, "default");
        assert!(prompt.starts_with("from SYSTEM.md"));
        assert!(!prompt.contains("default"));
    }

    #[test]
    fn test_build_system_prompt_appends_agents_md() {
        let ctx = LoadedContext {
            agents_files: vec![(
                PathBuf::from("/project/AGENTS.md"),
                "Be concise.".to_string(),
            )],
            ..Default::default()
        };
        let prompt = build_system_prompt(&ctx, None, "default prompt");
        assert!(prompt.contains("default prompt"));
        assert!(prompt.contains("Be concise."));
        assert!(prompt.contains("# Context from /project/AGENTS.md"));
    }

    #[test]
    fn test_build_system_prompt_appends_append_system_md() {
        let ctx = LoadedContext {
            append_system_prompt: Some("extra instructions".to_string()),
            ..Default::default()
        };
        let prompt = build_system_prompt(&ctx, None, "base");
        assert!(prompt.contains("base"));
        assert!(prompt.contains("extra instructions"));
    }

    #[test]
    fn test_build_system_prompt_full_assembly() {
        let ctx = LoadedContext {
            agents_files: vec![
                (PathBuf::from("/global/AGENTS.md"), "global rules".to_string()),
                (
                    PathBuf::from("/project/CLAUDE.md"),
                    "project rules".to_string(),
                ),
            ],
            system_prompt: Some("custom system".to_string()),
            append_system_prompt: Some("appended stuff".to_string()),
        };
        let prompt = build_system_prompt(&ctx, None, "unused default");
        // SYSTEM.md should be used as base
        assert!(prompt.starts_with("custom system"));
        // Both context files should be appended
        assert!(prompt.contains("global rules"));
        assert!(prompt.contains("project rules"));
        // APPEND_SYSTEM.md should be at the end
        assert!(prompt.contains("appended stuff"));
        // Default should not appear
        assert!(!prompt.contains("unused default"));
    }

    #[test]
    fn test_load_context_walks_ancestors() {
        let tmp = TempDir::new().unwrap();
        // Create a nested structure: tmp/a/b/c
        let dir_a = tmp.path().join("a");
        let dir_b = dir_a.join("b");
        let dir_c = dir_b.join("c");
        fs::create_dir_all(&dir_c).unwrap();

        // Place AGENTS.md in root and CLAUDE.md in the leaf
        fs::write(dir_a.join("AGENTS.md"), "root agents").unwrap();
        fs::write(dir_c.join("CLAUDE.md"), "leaf claude").unwrap();

        let ctx = load_context(&dir_c).unwrap();

        // Should have found both (ignoring global ~/.pi which may or may not exist)
        let paths: Vec<&Path> = ctx.agents_files.iter().map(|(p, _)| p.as_path()).collect();
        assert!(paths.contains(&dir_a.join("AGENTS.md").as_path()));
        assert!(paths.contains(&dir_c.join("CLAUDE.md").as_path()));

        // Root one should come before leaf (global -> ancestor -> local order)
        let root_idx = paths
            .iter()
            .position(|p| *p == dir_a.join("AGENTS.md"))
            .unwrap();
        let leaf_idx = paths
            .iter()
            .position(|p| *p == dir_c.join("CLAUDE.md"))
            .unwrap();
        assert!(root_idx < leaf_idx);
    }

    #[test]
    fn test_load_context_system_md_from_pi_dir() {
        let tmp = TempDir::new().unwrap();
        let pi_dir = tmp.path().join(".pi");
        fs::create_dir_all(&pi_dir).unwrap();
        fs::write(pi_dir.join("SYSTEM.md"), "custom system prompt").unwrap();
        fs::write(
            pi_dir.join("APPEND_SYSTEM.md"),
            "appended instructions",
        )
        .unwrap();

        let ctx = load_context(tmp.path()).unwrap();
        assert_eq!(
            ctx.system_prompt.as_deref(),
            Some("custom system prompt")
        );
        assert_eq!(
            ctx.append_system_prompt.as_deref(),
            Some("appended instructions")
        );
    }
}
