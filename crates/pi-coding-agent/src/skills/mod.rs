use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use pi_agent_core::{AgentTool, ToolContext, ToolResult};
use serde_json::Value;

const SKILL_FILE_NAME: &str = "SKILL.md";
const MAX_SKILL_CHARS: usize = 8_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub content: String,
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
}

#[derive(Debug, Default)]
pub struct ActiveSkills {
    names: BTreeSet<String>,
}

impl ActiveSkills {
    pub fn set(&mut self, name: &str) {
        self.names.insert(name.to_string());
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
                "## Skill: {}\nSource: {}\n\n{}",
                skill.name,
                skill.path.display(),
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

    let (name, description, content) = parse_skill_content(&source_file, &raw);
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

    let (name, description, content) = parse_skill_content(path, &raw);
    Ok(Some(Skill {
        name,
        description,
        path: path.to_path_buf(),
        content,
    }))
}

fn parse_skill_content(path: &Path, raw: &str) -> (String, String, String) {
    let mut name = String::new();
    let mut description = String::new();
    let mut body = raw.trim().to_string();

    if let Some(rest) = raw.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---\n") {
            let fm = &rest[..end];
            body = rest[(end + 5)..].trim().to_string();
            for line in fm.lines() {
                if let Some((k, v)) = line.split_once(':') {
                    let key = k.trim().to_lowercase();
                    let value = v.trim().trim_matches('"').trim_matches('\'').to_string();
                    match key.as_str() {
                        "name" => name = value,
                        "description" => description = value,
                        _ => {}
                    }
                }
            }
        }
    }

    if name.is_empty() {
        name = path
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
    if description.is_empty() {
        description = name.clone();
    }

    (name, description, body)
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
    fn parse_frontmatter_and_body() {
        let raw = "---\nname: rust-style\ndescription: Rust style guide\n---\nUse snake_case.";
        let path = PathBuf::from("/tmp/rust-style/SKILL.md");
        let (name, description, body) = parse_skill_content(&path, raw);
        assert_eq!(name, "rust-style");
        assert_eq!(description, "Rust style guide");
        assert_eq!(body, "Use snake_case.");
    }

    #[test]
    fn fallback_name_from_parent_dir() {
        let raw = "No frontmatter";
        let path = PathBuf::from("/tmp/code-review/SKILL.md");
        let (name, description, body) = parse_skill_content(&path, raw);
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
}
