use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use pi_agent_core::{AgentTool, ToolContext, ToolResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const SKILL_FILE_NAME: &str = "SKILL.md";
const MAX_SKILL_CHARS: usize = 8_000;

/// Full skill metadata from YAML frontmatter.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillMetadata {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub dependencies: Vec<String>,
    /// Source URL for remote skills (git repo, etc.)
    #[serde(default)]
    pub source: Option<String>,
    /// License information
    #[serde(default)]
    pub license: Option<String>,
    /// Minimum pi version required
    #[serde(default)]
    pub min_pi_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub content: String,
    /// Full metadata from frontmatter
    pub metadata: SkillMetadata,
}

impl Skill {
    /// Get formatted info for display.
    pub fn info(&self) -> String {
        let mut info = format!("Name: {}\n", self.name);
        info.push_str(&format!("Description: {}\n", self.description));
        if !self.metadata.version.is_empty() {
            info.push_str(&format!("Version: {}\n", self.metadata.version));
        }
        if !self.metadata.author.is_empty() {
            info.push_str(&format!("Author: {}\n", self.metadata.author));
        }
        if !self.metadata.tags.is_empty() {
            info.push_str(&format!("Tags: {}\n", self.metadata.tags.join(", ")));
        }
        if let Some(ref source) = self.metadata.source {
            info.push_str(&format!("Source: {}\n", source));
        }
        if let Some(ref license) = self.metadata.license {
            info.push_str(&format!("License: {}\n", license));
        }
        info
    }
}

#[derive(Debug, Default)]
pub struct SkillCatalog {
    skills: BTreeMap<String, Skill>,
    lookup: HashMap<String, String>,
}

impl SkillCatalog {
    pub fn discover(cwd: &Path) -> Result<Self> {
        let mut roots = Vec::new();
        if let Some(home) = std::env::var_os("HOME") {
            roots.push(PathBuf::from(home).join(".pi").join("skills"));
        }
        roots.push(cwd.join(".pi").join("skills"));
        Self::discover_from_roots(&roots)
    }

    fn discover_from_roots(roots: &[PathBuf]) -> Result<Self> {
        let mut catalog = SkillCatalog::default();

        // Later roots override earlier ones (project should override global).
        for root in roots {
            if !root.exists() {
                continue;
            }
            for skill_file in collect_skill_files(root)? {
                if let Some(skill) = parse_skill_file(&skill_file)? {
                    catalog.insert(skill);
                }
            }
        }

        Ok(catalog)
    }

    fn insert(&mut self, skill: Skill) {
        let key = skill.name.clone();
        self.lookup.insert(key.to_lowercase(), key.clone());
        self.skills.insert(key, skill);
    }

    pub fn upsert(&mut self, skill: Skill) {
        self.insert(skill);
    }

    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    pub fn len(&self) -> usize {
        self.skills.len()
    }

    pub fn get(&self, name: &str) -> Option<&Skill> {
        let key = self.lookup.get(&name.to_lowercase())?;
        self.skills.get(key)
    }

    pub fn names(&self) -> Vec<String> {
        self.skills.keys().cloned().collect()
    }

    pub fn skills(&self) -> Vec<Skill> {
        self.skills.values().cloned().collect()
    }

    /// Search skills by tag.
    pub fn by_tag(&self, tag: &str) -> Vec<&Skill> {
        let tag_lower = tag.to_lowercase();
        self.skills
            .values()
            .filter(|s| {
                s.metadata
                    .tags
                    .iter()
                    .any(|t| t.to_lowercase() == tag_lower)
            })
            .collect()
    }

    /// Search skills by name or description.
    pub fn search(&self, query: &str) -> Vec<&Skill> {
        let query_lower = query.to_lowercase();
        self.skills
            .values()
            .filter(|s| {
                s.name.to_lowercase().contains(&query_lower)
                    || s.description.to_lowercase().contains(&query_lower)
                    || s.metadata
                        .tags
                        .iter()
                        .any(|t| t.to_lowercase().contains(&query_lower))
            })
            .collect()
    }

    /// Get all unique tags in the catalog.
    pub fn all_tags(&self) -> Vec<String> {
        let mut tags: BTreeSet<String> = BTreeSet::new();
        for skill in self.skills.values() {
            for tag in &skill.metadata.tags {
                tags.insert(tag.clone());
            }
        }
        tags.into_iter().collect()
    }
}

#[derive(Debug, Default)]
pub struct ActiveSkills {
    names: BTreeSet<String>,
}

impl ActiveSkills {
    pub fn set(&mut self, name: &str) {
        self.names.insert(name.to_lowercase());
    }

    pub fn clear(&mut self) {
        self.names.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    pub fn list(&self) -> Vec<String> {
        self.names.iter().cloned().collect()
    }

    pub fn remove(&mut self, name: &str) {
        self.names.remove(&name.to_lowercase());
    }

    pub fn has(&self, name: &str) -> bool {
        self.names.contains(&name.to_lowercase())
    }
}

pub fn decorate_user_text(
    user_text: &str,
    catalog: &SkillCatalog,
    active: &ActiveSkills,
) -> String {
    if active.is_empty() {
        return user_text.to_string();
    }

    let mut sections = Vec::new();
    for name in active.list() {
        if let Some(skill) = catalog.get(&name) {
            let body: String = skill.content.chars().take(MAX_SKILL_CHARS).collect();
            sections.push(format!(
                "## Skill: {}\nSource: {}\nVersion: {}\n\n{}",
                skill.name,
                skill.path.display(),
                skill.metadata.version.as_str(),
                body
            ));
        }
    }

    if sections.is_empty() {
        return user_text.to_string();
    }

    let mut out = String::new();
    out.push_str("[Active skills]\n");
    out.push_str(&sections.join("\n\n"));
    out.push_str("\n[End active skills]\n\n");
    out.push_str(user_text);
    out
}

pub async fn register_skill_tools(agent: &pi_agent_core::Agent, catalog: &SkillCatalog) -> usize {
    let mut count = 0usize;
    for skill in catalog.skills() {
        let tool = SkillTool::from_skill(skill);
        agent.register_tool(Arc::new(tool)).await;
        count += 1;
    }
    count
}

pub async fn register_skill_tool(agent: &pi_agent_core::Agent, skill: Skill) {
    agent
        .register_tool(Arc::new(SkillTool::from_skill(skill)))
        .await;
}

pub fn install_skill_into_project(cwd: &Path, source: &Path) -> Result<Skill> {
    let destination_root = cwd.join(".pi").join("skills");
    install_skill(source, &destination_root)
}

/// Install a skill from a git repository URL.
pub async fn install_skill_from_git(
    cwd: &Path,
    git_url: &str,
    name: Option<&str>,
) -> Result<Skill> {
    let destination_root = cwd.join(".pi").join("skills");

    // Clone to temp directory
    let temp_dir = tempfile::tempdir()?;
    let clone_path = temp_dir.path().join("skill");

    let clone_path_str = clone_path.to_str().context("Invalid UTF-8 in clone path")?;
    let output = tokio::process::Command::new("git")
        .args(["clone", "--depth", "1", git_url, clone_path_str])
        .output()
        .await
        .context("Failed to execute git clone")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to clone repository: {}", stderr);
    }

    // Find SKILL.md in cloned repo
    let skill_file = if clone_path.join(SKILL_FILE_NAME).exists() {
        clone_path.join(SKILL_FILE_NAME)
    } else {
        // Search for any SKILL.md
        let mut found = None;
        for entry in walkdir::WalkDir::new(&clone_path).max_depth(2) {
            let entry = entry?;
            if entry.file_name() == SKILL_FILE_NAME {
                found = Some(entry.path().to_path_buf());
                break;
            }
        }
        found.context("No SKILL.md found in repository")?
    };

    let mut skill = install_skill(&skill_file, &destination_root)?;

    // Override name if specified
    if let Some(name) = name {
        skill.name = name.to_string();
        skill.metadata.name = name.to_string();
    }

    // Set source in metadata
    skill.metadata.source = Some(git_url.to_string());

    Ok(skill)
}

/// Install a skill from a remote URL (raw file).
pub async fn install_skill_from_url(cwd: &Path, url: &str, name: Option<&str>) -> Result<Skill> {
    let destination_root = cwd.join(".pi").join("skills");

    // Download skill file
    let response = reqwest::get(url)
        .await
        .context("Failed to download skill")?;
    if !response.status().is_success() {
        anyhow::bail!("Failed to download skill: HTTP {}", response.status());
    }
    let content = response.text().await.context("Failed to read response")?;

    // Parse to get name
    let (parsed_name, _, _) = parse_skill_content(Path::new("remote.md"), &content);
    let skill_name = name.unwrap_or(&parsed_name);

    // Create directory and write file
    let target_dir = destination_root.join(slugify(skill_name));
    fs::create_dir_all(&target_dir)?;
    let target_file = target_dir.join(SKILL_FILE_NAME);
    fs::write(&target_file, content)?;

    // Parse the installed skill
    let skill = parse_skill_file(&target_file)?.context("Failed to parse installed skill")?;

    Ok(skill)
}

fn install_skill(source: &Path, destination_root: &Path) -> Result<Skill> {
    let source_file = if source.is_dir() {
        source.join(SKILL_FILE_NAME)
    } else {
        source.to_path_buf()
    };

    if source_file
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n != SKILL_FILE_NAME)
        .unwrap_or(true)
    {
        anyhow::bail!(
            "source must be a '{}' file or directory containing '{}'",
            SKILL_FILE_NAME,
            SKILL_FILE_NAME
        );
    }

    let raw = fs::read_to_string(&source_file)
        .with_context(|| format!("failed to read skill file '{}'", source_file.display()))?;
    if raw.trim().is_empty() {
        anyhow::bail!("skill file is empty: {}", source_file.display());
    }

    let (name, description, content, metadata) = parse_skill_content_full(&source_file, &raw);
    let target_dir = destination_root.join(slugify(&name));
    fs::create_dir_all(&target_dir).with_context(|| {
        format!(
            "failed to create destination directory '{}'",
            target_dir.display()
        )
    })?;
    let target_file = target_dir.join(SKILL_FILE_NAME);
    fs::write(&target_file, raw)
        .with_context(|| format!("failed to write '{}'", target_file.display()))?;

    Ok(Skill {
        name,
        description,
        path: target_file,
        content,
        metadata,
    })
}

fn collect_skill_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(v) => v,
            Err(_) => continue,
        };
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file()
                && path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n == SKILL_FILE_NAME)
                    .unwrap_or(false)
            {
                files.push(path);
            }
        }
    }

    files.sort();
    Ok(files)
}

fn parse_skill_file(path: &Path) -> Result<Option<Skill>> {
    let raw = match fs::read_to_string(path) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };
    if raw.trim().is_empty() {
        return Ok(None);
    }

    let (name, description, content, metadata) = parse_skill_content_full(path, &raw);
    Ok(Some(Skill {
        name,
        description,
        path: path.to_path_buf(),
        content,
        metadata,
    }))
}

/// Parse skill content with full YAML frontmatter support.
fn parse_skill_content_full(path: &Path, raw: &str) -> (String, String, String, SkillMetadata) {
    let mut metadata = SkillMetadata::default();
    let mut body = raw.trim().to_string();

    // Try to parse YAML frontmatter
    if let Some(rest) = raw.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---\n") {
            let fm = &rest[..end];
            body = rest[(end + 5)..].trim().to_string();

            // Try full YAML parsing first
            match serde_yaml::from_str::<SkillMetadata>(fm) {
                Ok(parsed) => metadata = parsed,
                Err(_) => {
                    // Fallback to simple key:value parsing
                    for line in fm.lines() {
                        if let Some((k, v)) = line.split_once(':') {
                            let key = k.trim().to_lowercase();
                            let value = v.trim().trim_matches('"').trim_matches('\'').to_string();
                            match key.as_str() {
                                "name" => metadata.name = value,
                                "description" => metadata.description = value,
                                "version" => metadata.version = value,
                                "author" => metadata.author = value,
                                "license" => metadata.license = Some(value),
                                "source" => metadata.source = Some(value),
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
    }

    // Fallback for name
    if metadata.name.is_empty() {
        metadata.name = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
            .or_else(|| {
                path.file_stem()
                    .and_then(|n| n.to_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| "unnamed-skill".to_string());
    }

    let name = metadata.name.clone();
    let description = if metadata.description.is_empty() {
        name.clone()
    } else {
        metadata.description.clone()
    };
    metadata.description = description.clone();

    (name, description, body, metadata)
}

/// Legacy function for backward compatibility.
fn parse_skill_content(path: &Path, raw: &str) -> (String, String, String) {
    let (name, description, content, _) = parse_skill_content_full(path, raw);
    (name, description, content)
}

fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut prev_underscore = false;

    for ch in name.chars() {
        let normalized = if ch.is_ascii_alphanumeric() {
            ch.to_ascii_lowercase()
        } else {
            '_'
        };

        if normalized == '_' {
            if !prev_underscore && !out.is_empty() {
                out.push('_');
            }
            prev_underscore = true;
        } else {
            out.push(normalized);
            prev_underscore = false;
        }
    }

    while out.ends_with('_') {
        out.pop();
    }

    if out.is_empty() {
        "skill".to_string()
    } else {
        out
    }
}

#[derive(Debug, Clone)]
struct SkillTool {
    tool_name: String,
    skill_name: String,
    description: String,
    content: String,
    source: PathBuf,
}

impl SkillTool {
    fn from_skill(skill: Skill) -> Self {
        let tool_name = format!("skill_{}", slugify(&skill.name));
        let description = format!(
            "Load instructions from skill '{}' ({})",
            skill.name, skill.description
        );
        Self {
            tool_name,
            skill_name: skill.name,
            description,
            content: skill.content,
            source: skill.path,
        }
    }
}

#[async_trait]
impl AgentTool for SkillTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "Optional short task description for contextualized skill usage"
                }
            }
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> pi_agent_core::Result<ToolResult> {
        let task = args
            .get("task")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());

        let mut content = String::new();
        content.push_str(&format!(
            "Skill: {}\nSource: {}\n",
            self.skill_name,
            self.source.display()
        ));
        if let Some(task) = task {
            content.push_str(&format!("Task: {}\n", task));
        }
        content.push('\n');
        content.push_str(&self.content);

        Ok(ToolResult::success(content))
    }

    fn clone_boxed(&self) -> Box<dyn AgentTool> {
        Box::new(SkillTool {
            tool_name: self.tool_name.clone(),
            skill_name: self.skill_name.clone(),
            description: self.description.clone(),
            content: self.content.clone(),
            source: self.source.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn parse_yaml_frontmatter() {
        let raw = r#"---
name: rust-style
description: Rust style guide
version: "1.0.0"
author: "Test Author"
tags:
  - rust
  - style
  - formatting
---
Use snake_case."#;
        let path = PathBuf::from("/tmp/rust-style/SKILL.md");
        let (name, description, body, metadata) = parse_skill_content_full(&path, raw);
        assert_eq!(name, "rust-style");
        assert_eq!(description, "Rust style guide");
        assert_eq!(body, "Use snake_case.");
        assert_eq!(metadata.version, "1.0.0");
        assert_eq!(metadata.author, "Test Author");
        assert_eq!(metadata.tags, vec!["rust", "style", "formatting"]);
    }

    #[test]
    fn parse_frontmatter_and_body() {
        let raw = "---\nname: rust-style\ndescription: Rust style guide\n---\nUse snake_case.";
        let path = PathBuf::from("/tmp/rust-style/SKILL.md");
        let (name, description, body, _) = parse_skill_content_full(&path, raw);
        assert_eq!(name, "rust-style");
        assert_eq!(description, "Rust style guide");
        assert_eq!(body, "Use snake_case.");
    }

    #[test]
    fn fallback_name_from_parent_dir() {
        let raw = "No frontmatter";
        let path = PathBuf::from("/tmp/code-review/SKILL.md");
        let (name, description, body, _) = parse_skill_content_full(&path, raw);
        assert_eq!(name, "code-review");
        assert_eq!(description, "code-review");
        assert_eq!(body, "No frontmatter");
    }

    #[test]
    fn discover_overrides_global_with_project_skill() {
        let tmp = TempDir::new().unwrap();
        let global = tmp.path().join("global");
        let project = tmp.path().join("project");
        fs::create_dir_all(global.join("rust")).unwrap();
        fs::create_dir_all(project.join("rust")).unwrap();

        fs::write(
            global.join("rust").join("SKILL.md"),
            "---\nname: rust\ndescription: global\n---\nGlobal content",
        )
        .unwrap();
        fs::write(
            project.join("rust").join("SKILL.md"),
            "---\nname: rust\ndescription: project\n---\nProject content",
        )
        .unwrap();

        let catalog =
            SkillCatalog::discover_from_roots(&[global.to_path_buf(), project.to_path_buf()])
                .unwrap();
        let skill = catalog.get("rust").unwrap();
        assert_eq!(skill.description, "project");
        assert!(skill.content.contains("Project content"));
    }

    #[test]
    fn decorate_user_text_includes_active_skills() {
        let mut catalog = SkillCatalog::default();
        catalog.insert(Skill {
            name: "review".to_string(),
            description: "code review".to_string(),
            path: PathBuf::from("/tmp/review/SKILL.md"),
            content: "Prioritize high severity issues.".to_string(),
            metadata: SkillMetadata::default(),
        });
        let mut active = ActiveSkills::default();
        active.set("review");
        let out = decorate_user_text("Check src/", &catalog, &active);
        assert!(out.contains("[Active skills]"));
        assert!(out.contains("Prioritize high severity issues."));
        assert!(out.ends_with("Check src/"));
    }

    #[test]
    fn slugify_normalizes_name() {
        assert_eq!(slugify("Rust Review"), "rust_review");
        assert_eq!(slugify("C++/Style"), "c_style");
        assert_eq!(slugify("___"), "skill");
    }

    #[test]
    fn install_skill_writes_to_destination() {
        let tmp = TempDir::new().unwrap();
        let source_dir = tmp.path().join("source");
        let destination_root = tmp.path().join("dest").join(".pi").join("skills");
        fs::create_dir_all(&source_dir).unwrap();
        fs::write(
            source_dir.join("SKILL.md"),
            "---\nname: rust-review\ndescription: review rust\n---\nUse idiomatic Rust.",
        )
        .unwrap();

        let installed = install_skill(&source_dir, &destination_root).unwrap();
        assert_eq!(installed.name, "rust-review");
        assert!(installed.path.exists());
        let content = fs::read_to_string(&installed.path).unwrap();
        assert!(content.contains("Use idiomatic Rust."));
    }

    #[test]
    fn test_search_skills() {
        let mut catalog = SkillCatalog::default();
        catalog.insert(Skill {
            name: "rust-best-practices".to_string(),
            description: "Best practices for Rust".to_string(),
            path: PathBuf::from("/tmp/rust/SKILL.md"),
            content: "Content".to_string(),
            metadata: SkillMetadata {
                tags: vec!["rust".to_string(), "best-practices".to_string()],
                ..Default::default()
            },
        });
        catalog.insert(Skill {
            name: "python-style".to_string(),
            description: "Python style guide".to_string(),
            path: PathBuf::from("/tmp/python/SKILL.md"),
            content: "Content".to_string(),
            metadata: SkillMetadata {
                tags: vec!["python".to_string()],
                ..Default::default()
            },
        });

        let results = catalog.search("rust");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "rust-best-practices");

        let by_tag = catalog.by_tag("python");
        assert_eq!(by_tag.len(), 1);
        assert_eq!(by_tag[0].name, "python-style");
    }

    #[test]
    fn test_active_skills() {
        let mut active = ActiveSkills::default();
        active.set("skill1");
        active.set("skill2");

        assert!(active.has("skill1"));
        assert!(active.has("Skill1")); // case insensitive
        assert!(!active.has("skill3"));

        active.remove("skill1");
        assert!(!active.has("skill1"));
        assert_eq!(active.list().len(), 1);
    }
}
