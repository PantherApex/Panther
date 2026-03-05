use std::path::PathBuf;
use std::pin::Pin;
use std::future::Future;
use serde_json::{json, Value};
use super::Tool;

pub struct ReadSkillTool {
    skills_dir: PathBuf,
}

impl ReadSkillTool {
    pub fn new(skills_dir: PathBuf) -> Self {
        Self { skills_dir }
    }
}

impl Tool for ReadSkillTool {
    fn name(&self) -> &str {
        "read_skill"
    }

    fn description(&self) -> &str {
        "Load the full instructions for a named skill. Use this before executing a skill to get its complete documentation and usage guide. Pass the exact skill directory name as shown in the skills list."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The skill directory name (e.g. 'git-ops', 'docker-manager'). Must match exactly as shown in the skills list."
                }
            },
            "required": ["name"]
        })
    }

    fn execute<'a>(&'a self, args: Value) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
        Box::pin(async move {
            let name = match args.get("name").and_then(|v| v.as_str()) {
                Some(n) => n.to_string(),
                None => return "Error: 'name' parameter is required.".to_string(),
            };

            let sanitized: String = name.chars()
                .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
                .collect();
            if sanitized.is_empty() || sanitized != name {
                return format!("Error: invalid skill name '{}'. Use only alphanumeric characters, hyphens, and underscores.", name);
            }

            let skill_dir = self.skills_dir.join(&sanitized);
            if !skill_dir.exists() || !skill_dir.is_dir() {
                let mut available = Vec::new();
                if let Ok(mut entries) = tokio::fs::read_dir(&self.skills_dir).await {
                    while let Ok(Some(entry)) = entries.next_entry().await {
                        if entry.path().is_dir() {
                            available.push(entry.file_name().to_string_lossy().to_string());
                        }
                    }
                }
                if available.is_empty() {
                    return format!("Skill '{}' not found. No skills are currently installed.", sanitized);
                }
                return format!("Skill '{}' not found. Available skills: {}", sanitized, available.join(", "));
            }

            let skill_md = skill_dir.join("SKILL.md");
            if !skill_md.exists() {
                return format!("Skill directory '{}' exists but contains no SKILL.md.", sanitized);
            }

            match tokio::fs::read_to_string(&skill_md).await {
                Ok(content) => format!(
                    "# Skill: {}\nPath: {}\n\n{}",
                    sanitized,
                    skill_md.display(),
                    content
                ),
                Err(e) => format!("Error reading skill '{}': {}", sanitized, e),
            }
        })
    }
}
