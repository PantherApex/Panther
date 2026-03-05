use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use uuid::Uuid;

use providers::ProviderRouter;
use shared::bus::MessageBus;
use shared::types::{LLMMessage, LLMRequest};

use crate::context_builder::ContextBuilder;
use crate::tools::exec::ExecTool;
use crate::tools::filesystem::{ReadFileTool, WriteFileTool, EditFileTool, ListDirTool};
use crate::tools::message::MessageTool;
use crate::tools::web::{WebSearchTool, WebFetchTool};
use crate::tools::skill::ReadSkillTool;
use crate::tools::registry::ToolRegistry;

pub type AnnounceFn = Arc<dyn Fn(String, String, String) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync>;
pub type AnnounceSlot = Arc<tokio::sync::Mutex<Option<AnnounceFn>>>;

pub struct SubagentManager {
    providers: ProviderRouter,
    workspace: PathBuf,
    brave_api_key: Option<String>,
    exec_timeout_secs: u64,
    exec_path_append: String,
    temperature: f32,
    max_tokens: u32,
    max_iterations: usize,
    bus: MessageBus,
    announce: AnnounceSlot,
    running: Arc<Mutex<HashMap<String, JoinHandle<()>>>>,
    session_tasks: Arc<Mutex<HashMap<String, HashSet<String>>>>,
}

impl SubagentManager {
    pub fn new(
        providers: ProviderRouter,
        workspace: PathBuf,
        brave_api_key: Option<String>,
        exec_timeout_secs: u64,
        exec_path_append: String,
        temperature: f32,
        max_tokens: u32,
        max_iterations: usize,
        bus: MessageBus,
    ) -> Self {
        Self {
            providers,
            workspace,
            brave_api_key,
            exec_timeout_secs,
            exec_path_append,
            temperature,
            max_tokens,
            max_iterations,
            bus,
            announce: Arc::new(tokio::sync::Mutex::new(None)),
            running: Arc::new(Mutex::new(HashMap::new())),
            session_tasks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn set_announce(&self, f: AnnounceFn) {
        let slot = Arc::clone(&self.announce);
        tokio::spawn(async move {
            *slot.lock().await = Some(f);
        });
    }

    pub async fn spawn(
        self: Arc<Self>,
        task: String,
        label: Option<String>,
        origin_channel: String,
        origin_chat_id: String,
        session_key: String,
    ) -> String {
        let task_id = Uuid::new_v4().to_string()[..8].to_string();
        let display_label = label.unwrap_or_else(|| {
            let s: String = task.chars().take(40).collect();
            if task.len() > 40 { format!("{}...", s) } else { s }
        });

        let mgr = Arc::clone(&self);
        let task_clone = task.clone();
        let label_clone = display_label.clone();
        let task_id_clone = task_id.clone();
        let session_key_clone = session_key.clone();
        let origin_channel_clone = origin_channel.clone();
        let origin_chat_id_clone = origin_chat_id.clone();

        let handle = tokio::spawn(async move {
            mgr.run_subagent(task_id_clone.clone(), task_clone, label_clone, origin_channel_clone, origin_chat_id_clone, session_key_clone.clone()).await;
            mgr.running.lock().await.remove(&task_id_clone);
            let mut st = mgr.session_tasks.lock().await;
            if let Some(set) = st.get_mut(&session_key_clone) {
                set.remove(&task_id_clone);
            }
        });

        self.running.lock().await.insert(task_id.clone(), handle);
        self.session_tasks.lock().await.entry(session_key).or_default().insert(task_id.clone());

        format!("Subagent [{}] started (id: {}). I'll notify you when it completes.", display_label, task_id)
    }

    async fn run_subagent(
        &self,
        task_id: String,
        task: String,
        label: String,
        origin_channel: String,
        origin_chat_id: String,
        _session_key: String,
    ) {
        let working_dir = self.workspace.to_string_lossy().to_string();
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ExecTool::new(working_dir.clone(), self.exec_timeout_secs, self.exec_path_append.clone())));
        registry.register(Box::new(ReadFileTool));
        registry.register(Box::new(WriteFileTool));
        registry.register(Box::new(EditFileTool));
        registry.register(Box::new(ListDirTool));
        registry.register(Box::new(WebSearchTool::new(self.brave_api_key.clone())));
        registry.register(Box::new(WebFetchTool::new()));
        registry.register(Box::new(ReadSkillTool::new(self.workspace.join("skills"))));
        let msg_tool = Arc::new(MessageTool::new(self.bus.clone()));
        msg_tool.set_context(origin_channel.clone(), origin_chat_id.clone());
        registry.register_message_tool(msg_tool);

        let context_builder = ContextBuilder::new(self.workspace.clone());
        let runtime_ctx = context_builder.build_runtime_context(&origin_channel, &origin_chat_id);

        let system = format!(
            "# Subagent\n\n{}\n\nYou are a subagent spawned to complete a specific task. Stay focused. Your final response will be reported back to the user.\n\nWorkspace: {}",
            runtime_ctx,
            self.workspace.display()
        );

        let mut messages: Vec<LLMMessage> = vec![
            LLMMessage::system(system),
            LLMMessage::user(task.clone()),
        ];

        let max_iterations = self.max_iterations;
        let mut iterations = 0;
        let final_result: Option<String>;

        loop {
            let definitions = registry.definitions();
            let tools = if definitions.is_empty() { None } else { Some(definitions) };

            let request = LLMRequest {
                model: self.providers.active_model(),
                messages: messages.clone(),
                temperature: Some(self.temperature),
                max_tokens: Some(self.max_tokens),
                tools,
            };

            let response = match self.providers.chat(request).await {
                Ok(r) => r,
                Err(e) => {
                    final_result = Some(format!("Error: {}", e));
                    break;
                }
            };

            if response.finish_reason.as_deref() == Some("error") {
                final_result = response.content.or_else(|| Some("LLM returned an error.".to_string()));
                break;
            }

            if let Some(tool_calls) = response.tool_calls {
                messages.push(LLMMessage {
                    role: "assistant".to_string(),
                    content: response.content,
                    tool_calls: Some(tool_calls.clone()),
                    tool_call_id: None,
                    image_data: None,
                });
                for tc in &tool_calls {
                    let result = registry.execute(&tc.name, tc.arguments.clone()).await;
                    messages.push(LLMMessage::tool_result(tc.call_id.clone(), result));
                }
                iterations += 1;
                if iterations >= max_iterations {
                    final_result = Some(format!("Reached max iterations ({}) without completing.", max_iterations));
                    break;
                }
            } else {
                final_result = response.content;
                break;
            }
        }

        let result = final_result.unwrap_or_else(|| "Task completed.".to_string());
        let announce_content = format!(
            "[Subagent '{}' completed]\n\nTask: {}\n\nResult:\n{}\n\nSummarize this naturally for the user in 1-2 sentences.",
            label, task, result
        );

        if let Some(announce) = self.announce.lock().await.as_ref() {
            (announce)(origin_channel, origin_chat_id, announce_content).await;
        }

        let _ = task_id;
    }

    pub async fn cancel_by_session(&self, session_key: &str) -> usize {
        let task_ids: Vec<String> = {
            let st = self.session_tasks.lock().await;
            st.get(session_key).cloned().unwrap_or_default().into_iter().collect()
        };
        let mut running = self.running.lock().await;
        let mut count = 0;
        for id in &task_ids {
            if let Some(handle) = running.remove(id) {
                if !handle.is_finished() {
                    handle.abort();
                    count += 1;
                }
            }
        }
        self.session_tasks.lock().await.remove(session_key);
        count
    }
}
