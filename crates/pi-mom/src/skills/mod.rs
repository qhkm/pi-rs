use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A skill (custom CLI tool) that mom can create and use
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub command: String,
    pub path: PathBuf,
}

/// Load skills from a directory by scanning for `SKILL.md` files.
pub fn load_skills(dir: &Path) -> Vec<Skill> {
    let mut skills = Vec::new();
    collect_skill_files(dir, &mut skills);
    skills
}

/// Recursively walk `dir` looking for files named `SKILL.md`.
fn collect_skill_files(dir: &Path, out: &mut Vec<Skill>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_skill_files(&path, out);
        } else if path.file_name().and_then(|n| n.to_str()) == Some("SKILL.md") {
            if let Some(skill) = parse_skill_file(&path) {
                out.push(skill);
            }
        }
    }
}

/// Parse a `SKILL.md` file into a `Skill`.
///
/// The file may contain YAML frontmatter delimited by `---` lines with
/// `name`, `description`, and `command` fields.  If `name` is missing it
/// falls back to the parent directory name.
fn parse_skill_file(path: &Path) -> Option<Skill> {
    let content = std::fs::read_to_string(path).ok()?;

    let mut name = None;
    let mut description = String::new();
    let mut command = String::new();

    // Check for YAML frontmatter (between first and second `---`)
    if content.starts_with("---") {
        let rest = &content[3..];
        if let Some(end) = rest.find("---") {
            let frontmatter = &rest[..end];
            for line in frontmatter.lines() {
                let line = line.trim();
                if let Some(val) = line.strip_prefix("name:") {
                    name = Some(val.trim().trim_matches('"').trim_matches('\'').to_string());
                } else if let Some(val) = line.strip_prefix("description:") {
                    description = val.trim().trim_matches('"').trim_matches('\'').to_string();
                } else if let Some(val) = line.strip_prefix("command:") {
                    command = val.trim().trim_matches('"').trim_matches('\'').to_string();
                }
            }
        }
    }

    // Fallback name from parent directory
    let name = name.unwrap_or_else(|| {
        path.parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("unnamed")
            .to_string()
    });

    Some(Skill {
        name,
        description,
        command,
        path: path.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn load_skills_populated_dir() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("my-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: greet\ndescription: Say hello\ncommand: echo hello\n---\n# Greet\n",
        )
        .unwrap();

        let skills = load_skills(dir.path());
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "greet");
        assert_eq!(skills[0].description, "Say hello");
        assert_eq!(skills[0].command, "echo hello");
    }

    #[test]
    fn load_skills_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let skills = load_skills(dir.path());
        assert!(skills.is_empty());
    }

    #[test]
    fn load_skills_nonexistent_dir() {
        let skills = load_skills(Path::new("/nonexistent/path/xyzzy"));
        assert!(skills.is_empty());
    }
}
