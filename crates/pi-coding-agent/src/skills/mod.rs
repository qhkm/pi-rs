use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

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
}
