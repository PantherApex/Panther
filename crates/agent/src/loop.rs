use std::path::PathBuf;

use providers::ProviderRouter;
use shared::errors::PantherResult;
use shared::types::{LLMMessage, LLMRequest, ToolCall};
use crate::tools::registry::ToolRegistry;

pub type ProgressCallback = Box<dyn Fn(String, bool) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync>;
pub type FileDispatchCallback = Box<dyn Fn(PathBuf) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync>;

enum ToolCategory {
    CodeExecution,
    FileRead,
    FileWrite,
    FileEdit,
    DirectoryList,
    MediaCapture,
    WebSearch,
    WebFetch,
    Communication,
    FileSend,
    Scheduling,
    AgentSpawn,
    SkillLoad,
    SystemInspect,
    ClipboardAccess,
    MemoryOperation,
    ExternalService,
}

impl ToolCategory {
    fn classify(name: &str) -> Self {
        if name == "exec" { return Self::CodeExecution; }
        if name == "read_file" { return Self::FileRead; }
        if name == "write_file" { return Self::FileWrite; }
        if name == "edit_file" { return Self::FileEdit; }
        if name == "list_dir" { return Self::DirectoryList; }
        if name == "capture_media" { return Self::MediaCapture; }
        if name == "web_search" { return Self::WebSearch; }
        if name == "web_fetch" { return Self::WebFetch; }
        if name == "message" { return Self::Communication; }
        if name == "send_file" { return Self::FileSend; }
        if name == "cron" { return Self::Scheduling; }
        if name == "spawn" { return Self::AgentSpawn; }
        if name == "read_skill" { return Self::SkillLoad; }
        if name == "system_info" { return Self::SystemInspect; }
        if name == "clipboard" { return Self::ClipboardAccess; }
        if name.contains("memory") || name.contains("remember") { return Self::MemoryOperation; }
        Self::ExternalService
    }

    fn phase_label(&self, args: &serde_json::Value) -> String {
        let primary = extract_primary_arg(args);
        match self {
            Self::CodeExecution => match primary {
                Some(cmd) => format!("⚡ Executing: {}", truncate(cmd, 48)),
                None => "⚡ Executing command...".to_string(),
            },
            Self::FileRead => match primary {
                Some(path) => format!("📖 Reading: {}", truncate(basename(path), 40)),
                None => "📖 Reading file...".to_string(),
            },
            Self::FileWrite => match primary {
                Some(path) => format!("📝 Writing: {}", truncate(basename(path), 40)),
                None => "📝 Writing file...".to_string(),
            },
            Self::FileEdit => match primary {
                Some(path) => format!("✏️ Editing: {}", truncate(basename(path), 40)),
                None => "✏️ Editing file...".to_string(),
            },
            Self::DirectoryList => match primary {
                Some(path) => format!("📂 Listing: {}", truncate(basename(path), 40)),
                None => "📂 Listing directory...".to_string(),
            },
            Self::MediaCapture => "📸 Capturing media...".to_string(),
            Self::WebSearch => match primary {
                Some(query) => format!("🌐 Searching: {}", truncate(query, 48)),
                None => "🌐 Searching the web...".to_string(),
            },
            Self::WebFetch => match primary {
                Some(url) => format!("🌐 Fetching: {}", truncate(url, 48)),
                None => "🌐 Fetching URL...".to_string(),
            },
            Self::Communication => "💬 Sending message...".to_string(),
            Self::FileSend => "📤 Sending file...".to_string(),
            Self::Scheduling => "⏰ Scheduling task...".to_string(),
            Self::AgentSpawn => "🤖 Spawning sub-agent...".to_string(),
            Self::SkillLoad => match primary {
                Some(name) => format!("🛠️ Loading skill: {}", truncate(name, 40)),
                None => "🛠️ Loading skill...".to_string(),
            },
            Self::SystemInspect => "💻 Inspecting system...".to_string(),
            Self::ClipboardAccess => "📋 Accessing clipboard...".to_string(),
            Self::MemoryOperation => "💾 Updating memory...".to_string(),
            Self::ExternalService => "⚙️ Calling service...".to_string(),
        }
    }
}

fn extract_primary_arg(args: &serde_json::Value) -> Option<&str> {
    let obj = args.as_object()?;
    for key in &["command", "query", "url", "path", "text", "name", "skill_name", "message"] {
        if let Some(v) = obj.get(*key).and_then(|v| v.as_str()) {
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    obj.values().find_map(|v| v.as_str().filter(|s| !s.is_empty()))
}

fn truncate(s: &str, max: usize) -> &str {
    let boundary = s.char_indices()
        .nth(max)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    if boundary < s.len() {
        &s[..boundary]
    } else {
        s
    }
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next()
        .and_then(|s| s.rsplit('\\').next())
        .unwrap_or(path)
}

fn tool_phase_label(tc: &ToolCall) -> String {
    ToolCategory::classify(&tc.name).phase_label(&tc.arguments)
}

fn extract_file_markers(raw: &str) -> (String, Vec<PathBuf>) {
    const MARKER_PREFIX: &str = "[PANTHER_FILE:";
    let mut paths = Vec::new();
    let mut clean = raw.to_string();
    loop {
        match clean.find(MARKER_PREFIX) {
            Some(start) => match clean[start..].find(']') {
                Some(rel_end) => {
                    let path_str = clean[start + MARKER_PREFIX.len()..start + rel_end].to_string();
                    let abs_end = start + rel_end + 1;
                    clean = format!("{}{}", &clean[..start], &clean[abs_end..]);
                    paths.push(PathBuf::from(path_str));
                }
                None => break,
            },
            None => break,
        }
    }
    (clean.trim().to_string(), paths)
}

pub async fn run(
    providers: &ProviderRouter,
    registry: &ToolRegistry,
    mut messages: Vec<LLMMessage>,
    max_iterations: usize,
    max_tokens: u32,
    temperature: f32,
    on_progress: Option<&ProgressCallback>,
    on_file: Option<&FileDispatchCallback>,
) -> PantherResult<(Option<String>, Vec<LLMMessage>, bool)> {
    let mut iterations = 0;

    loop {
        if let Some(cb) = on_progress {
            let phase = if iterations == 0 {
                "🧠 Thinking...".to_string()
            } else {
                "🔍 Analyzing results...".to_string()
            };
            cb(phase, false).await;
        }

        let definitions = registry.definitions();
        let tools = if definitions.is_empty() { None } else { Some(definitions) };

        let request = LLMRequest {
            model: providers.active_model(),
            messages: messages.clone(),
            temperature: Some(temperature),
            max_tokens: Some(max_tokens),
            tools,
        };

        let response = providers.chat(request).await?;

        if let Some(ref finish) = response.finish_reason {
            if finish == "error" {
                let content = response.content.unwrap_or_else(|| "LLM returned an error.".to_string());
                return Ok((Some(content), messages, true));
            }
        }

        if let Some(tool_calls) = response.tool_calls {
            if let Some(cb) = on_progress {
                if let Some(ref text) = response.content {
                    let trimmed = text.trim().to_string();
                    if !trimmed.is_empty() {
                        cb(trimmed, false).await;
                    }
                }
            }

            let assistant_msg = LLMMessage {
                role: "assistant".to_string(),
                content: response.content,
                tool_calls: Some(tool_calls.clone()),
                tool_call_id: None,
                image_data: None,
            };
            messages.push(assistant_msg);

            for tc in &tool_calls {
                if let Some(cb) = on_progress {
                    cb(tool_phase_label(tc), true).await;
                }
                let raw_result = registry.execute(&tc.name, tc.arguments.clone()).await;
                let (stripped_result, file_paths) = extract_file_markers(&raw_result);
                let mut dispatched: usize = 0;
                if let Some(file_cb) = on_file {
                    for path in file_paths {
                        if path.exists() {
                            file_cb(path).await;
                            dispatched += 1;
                        }
                    }
                }
                let tool_result = if dispatched > 0 {
                    let base = stripped_result.trim_end_matches('.');
                    format!("{}. {} file(s) delivered directly to chat — no further send action needed.", base.trim(), dispatched)
                } else {
                    stripped_result
                };
                messages.push(LLMMessage::tool_result(tc.call_id.clone(), tool_result));
            }

            iterations += 1;
            if iterations >= max_iterations {
                let last_content = messages.iter().rev()
                    .find(|m| m.role == "assistant")
                    .and_then(|m| m.content.clone());
                let fallback = format!(
                    "I reached the maximum number of steps ({}) without completing the task. Try breaking it into smaller steps.",
                    max_iterations
                );
                return Ok((Some(last_content.unwrap_or(fallback)), messages, false));
            }
        } else {
            messages.push(LLMMessage::assistant(
                response.content.clone().unwrap_or_default()
            ));
            return Ok((response.content, messages, false));
        }
    }
}
