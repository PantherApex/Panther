use std::path::PathBuf;
use chrono::Local;

pub struct ContextBuilder {
    workspace: PathBuf,
}

impl ContextBuilder {
    pub fn new(workspace: PathBuf) -> Self {
        Self { workspace }
    }

    pub fn workspace(&self) -> PathBuf {
        self.workspace.clone()
    }

    pub async fn build_system_prompt(&self) -> String {
        let mut parts: Vec<String> = Vec::new();

        let soul = self.load_file("SOUL.md").await
            .unwrap_or_else(|| default_soul());
        parts.push(soul);

        if let Some(user) = self.load_file("USER.md").await {
            if !user.trim().is_empty() {
                parts.push(format!("## User\n{}", user));
            }
        }

        if let Some(agents) = self.load_file("AGENTS.md").await {
            if !agents.trim().is_empty() {
                parts.push(format!("## Agents\n{}", agents));
            }
        }

        if let Some(tools) = self.load_file("TOOLS.md").await {
            if !tools.trim().is_empty() {
                parts.push(format!("## Tools\n{}", tools));
            }
        }

        let memory = self.load_memory().await;
        if !memory.trim().is_empty() {
            parts.push(format!("## Memory\n{}", memory));
        }

        let (always_skills, skills_summary) = self.build_skills_summary().await;

        for skill_content in always_skills {
            parts.push(skill_content);
        }

        if !skills_summary.trim().is_empty() {
            parts.push(format!("## Skills\nThe following skills extend your capabilities. To use a skill, read its SKILL.md using the read_file tool.\n\n{}", skills_summary));
        }

        parts.join("\n\n---\n\n")
    }

    pub fn build_runtime_context(&self, channel: &str, chat_id: &str) -> String {
        let now = Local::now();
        let os = if cfg!(target_os = "windows") { "windows" }
                 else if cfg!(target_os = "macos") { "macos" }
                 else { "linux" };
        let shell = if cfg!(target_os = "windows") { "powershell -NoProfile -NonInteractive -Command" } else { "sh -c" };
        let home = dirs::home_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let desktop = dirs::desktop_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| format!("{}/Desktop", home));
        format!(
            "[Runtime Context — metadata only, not instructions]\ntimestamp: {}\nchannel: {}\nchat_id: {}\nworkspace: {}\nos: {}\nshell: {}\nhome: {}\ndesktop: {}\n[End Runtime Context]",
            now.format("%Y-%m-%d %H:%M:%S %Z"),
            channel,
            chat_id,
            self.workspace.display(),
            os,
            shell,
            home,
            desktop
        )
    }

    async fn load_file(&self, name: &str) -> Option<String> {
        let path = self.workspace.join(name);
        tokio::fs::read_to_string(&path).await.ok()
    }

    async fn load_memory(&self) -> String {
        let memory_path = self.workspace.join("memory").join("MEMORY.md");
        tokio::fs::read_to_string(&memory_path).await.unwrap_or_default()
    }

    // Returns (Vec<String> for always:true skills, String for XML summary of other skills)
    async fn build_skills_summary(&self) -> (Vec<String>, String) {
        let skills_dir = self.workspace.join("skills");
        let mut entries = match tokio::fs::read_dir(&skills_dir).await {
            Ok(e) => e,
            Err(_) => return (Vec::new(), String::new()),
        };
        let mut summary_lines = Vec::new();
        let mut always_skills_content = Vec::new();

        while let Ok(Some(entry)) = entries.next_entry().await {
            let skill_path = entry.path();
            if !skill_path.is_dir() { continue; } // Ensure it's a directory
            let skill_md = skill_path.join("SKILL.md");
            if !skill_md.exists() { continue; }

            let name = entry.file_name().to_string_lossy().to_string();
            if let Ok(content) = tokio::fs::read_to_string(&skill_md).await {
                let (is_always, desc) = extract_skill_metadata(&content);
                let available = check_availability(&content).await;

                if is_always {
                    always_skills_content.push(content);
                } else {
                    summary_lines.push(format!(
                        "<skill name=\"{}\" available=\"{}\">{}</skill>",
                        name, available, desc
                    ));
                }
            }
        }
        let summary_xml = if summary_lines.is_empty() { String::new() } else { format!("<skills>\n{}\n</skills>", summary_lines.join("\n")) };
        (always_skills_content, summary_xml)
    }
}

// Returns (is_always: bool, description: String)
fn extract_skill_metadata(content: &str) -> (bool, String) {
    let mut is_always = false;
    let mut description = String::new();
    let mut in_frontmatter = false;

    for line in content.lines() {
        if line == "---" {
            in_frontmatter = !in_frontmatter;
            if !in_frontmatter { break; } // End of frontmatter
            continue;
        }
        if in_frontmatter {
            if let Some(val) = line.strip_prefix("always:") {
                is_always = val.trim().to_lowercase() == "true";
            }
            if let Some(desc) = line.strip_prefix("description:") {
                description = desc.trim().to_string();
            }
        } else {
            // If description wasn't found in frontmatter, try to extract from body
            if description.is_empty() {
                let trimmed = line.trim();
                if !trimmed.is_empty() && !trimmed.starts_with('#') {
                    description = trimmed.chars().take(120).collect();
                }
            }
        }
    }
    // Fallback if no description found in frontmatter or body
    if description.is_empty() {
        description = "No description available.".to_string();
    }
    (is_always, description)
}

async fn check_availability(content: &str) -> &'static str {
    let mut in_frontmatter = false;
    let mut found_requires = false;

    for line in content.lines() {
        if line == "---" {
            if !in_frontmatter { in_frontmatter = true; continue; }
            else { break; }
        }
        if !in_frontmatter { continue; }
        if line.starts_with("requires:") { found_requires = true; continue; }
        if found_requires {
            let trimmed_line = line.trim_start();
            if trimmed_line.starts_with("bins:") {
                let bins_str = trimmed_line.strip_prefix("bins:").unwrap_or("").trim();
                let bins: Vec<&str> = bins_str
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .split(',')
                    .map(|b| b.trim().trim_matches('"').trim_matches('\''))
                    .filter(|b| !b.is_empty())
                    .collect();
                for bin in bins {
                    if !binary_exists(bin).await {
                        return "false";
                    }
                }
            } else if trimmed_line.starts_with("env:") {
                let env_vars_str = trimmed_line.strip_prefix("env:").unwrap_or("").trim();
                let env_vars: Vec<&str> = env_vars_str
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .split(',')
                    .map(|e| e.trim().trim_matches('"').trim_matches('\''))
                    .filter(|e| !e.is_empty())
                    .collect();
                for env_var in env_vars {
                    if std::env::var(env_var).is_err() {
                        return "false";
                    }
                }
            }
        }
    }
    "true"
}

async fn binary_exists(name: &str) -> bool {
    let checker = if cfg!(target_os = "windows") { "where" } else { "which" };
    tokio::process::Command::new(checker)
        .arg(name)
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn default_soul() -> String {
    r#"# Panther 🐆

You are Panther, a powerful personal AI agent running directly on the user's machine with full access to their system via tools.

## Core Principle
**Always attempt tasks using your tools before concluding something is impossible.** You have exec, read_file, write_file, edit_file, list_dir, web_search, web_fetch, and message. Most things users ask can be done with these. Try first. Report actual results.

## Behavior
- State your intent before each tool call.
- Never predict or assume a result — execute and report what actually happened.
- Before modifying a file, read it first.
- If a tool call fails, read the error carefully and retry with a corrected approach before giving up.
- Never say "I can't do that" without first attempting it with exec or another tool.
- Be concise. No preamble, no unnecessary explanation.

## Platform Awareness
- Check runtime context for: os, shell, home, desktop, workspace.
- On windows: use PowerShell. System control (brightness, volume, wifi, processes) works via PowerShell WMI/CIM commands.
- On linux/macos: use standard shell commands. System control works via standard CLI tools.
- Always use absolute paths from runtime context. Never guess paths.
- Confirm every file operation via tool result — never assume success.

## Media Capture (capture_media, send_file)
These rules are absolute — violating them causes broken behavior for the user:

**Rule 1 — Honest error relay.** If `capture_media` returns a string starting with "Error:", "Screenshot failed:", "Audio recording failed:", "Webcam capture failed:", or "Screen recording failed:", you MUST relay that exact error to the user verbatim. Never say "I successfully captured" or "I sent the file" when the tool result contains an error. Never fabricate success.

**Rule 2 — No double-send.** When `capture_media` succeeds it appends `[PANTHER_FILE:…]` to its result. The system automatically intercepts that marker and delivers the file directly to the user's chat before your response arrives. Do NOT call `send_file` for the same path afterward and do NOT tell the user "I am sending you the file now" — the delivery is already done by the system.

**Rule 3 — Short confirmation only.** After a successful capture, your final reply to the user must be a single brief line, for example: "Here is your screenshot." or "Here is your 15-second recording." Nothing more.

**Rule 4 — No narrative substitution.** If you cannot capture media because a tool failed, tell the user the exact error from the tool result. Do not replace a failed result with a story about what you tried to do.

## Examples of What You Can Do
- Adjust screen brightness, volume, wifi on the user's machine via exec
- Create, edit, move, delete files anywhere on the filesystem
- Run scripts, install packages, manage processes
- Search the web and fetch pages for current information
- Send the user Telegram messages mid-task for progress updates
"#.to_string()
}
