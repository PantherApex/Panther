use std::path::PathBuf;
use serde_json::{json, Value};
use shared::types::{LLMMessage, LLMRequest, ToolDefinition};
use shared::errors::PantherResult;
use providers::ProviderRouter;

use crate::session::Session;

pub struct AgentMemory {
    memory_file: PathBuf,
    history_file: PathBuf,
}

impl AgentMemory {
    pub fn new(workspace: &PathBuf) -> Self {
        let memory_dir = workspace.join("memory");
        Self {
            memory_file: memory_dir.join("MEMORY.md"),
            history_file: memory_dir.join("HISTORY.md"),
        }
    }

    pub async fn read_long_term(&self) -> String {
        tokio::fs::read_to_string(&self.memory_file)
            .await
            .unwrap_or_default()
    }

    pub async fn write_long_term(&self, content: &str) -> PantherResult<()> {
        if let Some(parent) = self.memory_file.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&self.memory_file, content).await?;
        Ok(())
    }

    pub async fn append_history(&self, entry: &str) -> PantherResult<()> {
        if let Some(parent) = self.history_file.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let line = format!("{}\n\n", entry.trim_end());
        tokio::fs::write(
            &self.history_file,
            {
                let existing = tokio::fs::read_to_string(&self.history_file)
                    .await
                    .unwrap_or_default();
                format!("{}{}", existing, line)
            },
        ).await?;
        Ok(())
    }

    pub async fn consolidate(
        &self,
        session: &mut Session,
        providers: &ProviderRouter,
        memory_window: usize,
        archive_all: bool,
    ) -> bool {
        let (old_messages, keep_count) = if archive_all {
            (session.messages.clone(), 0)
        } else {
            let keep_count = memory_window / 2;
            if session.messages.len() <= keep_count {
                return true;
            }
            if session.unconsolidated_count() == 0 {
                return true;
            }
            let end = session.messages.len().saturating_sub(keep_count);
            let old = session.messages[session.last_consolidated..end].to_vec();
            if old.is_empty() {
                return true;
            }
            (old, keep_count)
        };

        let lines: Vec<String> = {
            let mut result = Vec::new();
            let mut i = 0;
            while i < old_messages.len() {
                let m = &old_messages[i];
                if m.role == "assistant" {
                    let tools_used: Vec<String> = m.tool_calls.as_ref()
                        .map(|calls| calls.iter().map(|tc| tc.name.clone()).collect())
                        .unwrap_or_default();
                    if let Some(content) = m.content.as_deref().filter(|c| !c.is_empty()) {
                        if tools_used.is_empty() {
                            result.push(format!("ASSISTANT: {}", content));
                        } else {
                            result.push(format!("ASSISTANT [tools: {}]: {}", tools_used.join(", "), content));
                        }
                    } else if !tools_used.is_empty() {
                        result.push(format!("ASSISTANT [tools: {}]", tools_used.join(", ")));
                    }
                } else if let Some(content) = m.content.as_deref().filter(|c| !c.is_empty()) {
                    result.push(format!("{}: {}", m.role.to_uppercase(), content));
                }
                i += 1;
            }
            result
        };

        if lines.is_empty() {
            return true;
        }

        let current_memory = self.read_long_term().await;

        let prompt = format!(
            "Process this conversation and call the save_memory tool with your consolidation.\n\n## Current Long-term Memory\n{}\n\n## Conversation to Process\n{}",
            if current_memory.is_empty() { "(empty)".to_string() } else { current_memory.clone() },
            lines.join("\n")
        );

        let save_memory_tool = ToolDefinition {
            name: "save_memory".to_string(),
            description: "Save the memory consolidation result to persistent storage.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "history_entry": {
                        "type": "string",
                        "description": "A paragraph (2-5 sentences) summarizing key events/decisions/topics from this conversation. Start with [YYYY-MM-DD HH:MM]. Include detail useful for grep search."
                    },
                    "memory_update": {
                        "type": "string",
                        "description": "Full updated long-term memory as markdown. Include all existing facts plus new ones learned. Return unchanged if nothing new."
                    }
                },
                "required": ["history_entry", "memory_update"]
            }),
        };

        let request = LLMRequest {
            model: providers.active_model(),
            messages: vec![
                LLMMessage::system("You are a memory consolidation agent. Your only job is to call the save_memory tool with a concise summary of the conversation.".to_string()),
                LLMMessage::user(prompt),
            ],
            temperature: Some(0.1),
            max_tokens: Some(2048),
            tools: Some(vec![save_memory_tool]),
        };

        let response = match providers.chat(request).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Memory consolidation LLM call failed: {}", e);
                return false;
            }
        };

        let tool_calls = match response.tool_calls {
            Some(calls) if !calls.is_empty() => calls,
            _ => {
                eprintln!("Memory consolidation: LLM did not call save_memory");
                return false;
            }
        };

        let args: Value = match &tool_calls[0].arguments {
            v if v.is_string() => {
                match serde_json::from_str(v.as_str().unwrap_or("{}")) {
                    Ok(parsed) => parsed,
                    Err(_) => return false,
                }
            }
            v => v.clone(),
        };

        if let Some(entry) = args.get("history_entry").and_then(|v| v.as_str()) {
            let _ = self.append_history(entry).await;
        }

        if let Some(update) = args.get("memory_update").and_then(|v| v.as_str()) {
            if update != current_memory {
                let _ = self.write_long_term(update).await;
            }
        }

        if archive_all {
            session.last_consolidated = 0;
        } else {
            session.last_consolidated = session.messages.len() - keep_count;
        }

        true
    }
}
