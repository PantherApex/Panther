use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};
use tokio::task::JoinHandle;

use providers::ProviderRouter;
use shared::bus::{InboundMessage, MessageBus, OutboundMessage};
use shared::errors::{PantherError, PantherResult};
use shared::events::EventBus;
use shared::types::LLMMessage;

use crate::context_builder::ContextBuilder;
use crate::file_tracker::TurnFileTracker;
use crate::memory::AgentMemory;
use crate::session::SessionStore;
use crate::subagent::SubagentManager;
use crate::tools::registry::ToolRegistry;
use crate::r#loop::{FileDispatchCallback, ProgressCallback};
use crate::r#loop::run as run_loop;

type TaskHandle = JoinHandle<()>;

pub struct Agent {
    pub providers: ProviderRouter,
    pub registry: Arc<Mutex<ToolRegistry>>,
    pub session_store: SessionStore,
    pub context_builder: ContextBuilder,
    pub memory: AgentMemory,
    pub bus: MessageBus,
    pub event_bus: EventBus,
    pub max_iterations: usize,
    pub max_tokens: u32,
    pub memory_window: usize,
    pub temperature: f32,
    pub send_progress: bool,
    turn_file_tracker: Arc<TurnFileTracker>,
    consolidating: Arc<Mutex<HashSet<String>>>,
    active_tasks: Arc<Mutex<HashMap<String, Vec<TaskHandle>>>>,
    session_locks: Arc<Mutex<HashMap<String, Arc<Semaphore>>>>,
    pub subagents: Option<Arc<SubagentManager>>,
}

impl Agent {
    pub fn new(
        providers: ProviderRouter,
        registry: ToolRegistry,
        session_store: SessionStore,
        context_builder: ContextBuilder,
        workspace: PathBuf,
        bus: MessageBus,
        event_bus: EventBus,
        max_iterations: usize,
        max_tokens: u32,
        temperature: f32,
        memory_window: usize,
        send_progress: bool,
    ) -> Self {
        Self {
            memory: AgentMemory::new(&workspace),
            providers,
            registry: Arc::new(Mutex::new(registry)),
            session_store,
            context_builder,
            bus,
            event_bus,
            max_iterations,
            max_tokens,
            temperature,
            memory_window,
            send_progress,
            turn_file_tracker: Arc::new(TurnFileTracker::new()),
            consolidating: Arc::new(Mutex::new(HashSet::new())),
            active_tasks: Arc::new(Mutex::new(HashMap::new())),
            session_locks: Arc::new(Mutex::new(HashMap::new())),
            subagents: None,
        }
    }

    pub fn with_subagents(mut self, mgr: Arc<SubagentManager>) -> Self {
        self.subagents = Some(mgr);
        self
    }

    async fn session_semaphore(&self, key: &str) -> Arc<Semaphore> {
        let mut locks = self.session_locks.lock().await;
        locks.entry(key.to_string())
            .or_insert_with(|| Arc::new(Semaphore::new(1)))
            .clone()
    }

    pub async fn run(self: Arc<Self>) {
        while let Some(msg) = self.bus.consume_inbound().await {
            let session_key = msg.session_key();
            let agent = Arc::clone(&self);
            let task = tokio::spawn(async move {
                agent.dispatch_message(msg).await;
            });
            self.active_tasks.lock().await
                .entry(session_key)
                .or_default()
                .push(task);
        }
    }

    async fn dispatch_message(self: Arc<Self>, msg: InboundMessage) {
        let session_key = msg.session_key();
        let sem = self.session_semaphore(&session_key).await;

        let _permit = match sem.acquire().await {
            Ok(p) => p,
            Err(_) => return,
        };

        let response = match self.process_message(msg).await {
            Ok(Some(out)) => out,
            Ok(None) => return,
            Err(e) => {
                eprintln!("[panther:agent] dispatch error: {}", e);
                return;
            }
        };

        self.bus.publish_outbound(response).await;
    }

    pub async fn stop(&self, session_key: &str) -> usize {
        let mut tasks = self.active_tasks.lock().await;
        let list = tasks.remove(session_key).unwrap_or_default();
        let count = list.iter().filter(|t| !t.is_finished()).count();
        for handle in list {
            handle.abort();
        }
        let subagent_count = if let Some(ref mgr) = self.subagents {
            mgr.cancel_by_session(session_key).await
        } else {
            0
        };
        count + subagent_count
    }

    async fn process_message(&self, msg: InboundMessage) -> PantherResult<Option<OutboundMessage>> {
        let session_key = msg.session_key();
        let InboundMessage { channel, sender_id: _, chat_id, content, media_path, image_b64, session_key_override: _ } = msg;

        {
            let registry = self.registry.lock().await;
            if let Some(tool) = registry.get_send_file_tool() {
                tool.set_context(channel.clone(), chat_id.clone());
            }
            if let Some(tool) = registry.get_message_tool() {
                tool.set_context(channel.clone(), chat_id.clone());
            }
            if let Some(tool) = registry.get_cron_tool() {
                tool.set_context(channel.clone(), chat_id.clone());
            }
            if let Some(tool) = registry.get_spawn_tool() {
                tool.set_context(channel.clone(), chat_id.clone());
            }
        }

        let mut session = self.session_store.get_or_create(&session_key).await;

        if session.unconsolidated_count() >= self.memory_window {
            let already = self.consolidating.lock().await.contains(&session_key);
            if !already {
                self.consolidating.lock().await.insert(session_key.clone());
                let providers = self.providers.clone();
                let memory_window = self.memory_window;
                let consolidating_set = Arc::clone(&self.consolidating);
                let session_store = self.session_store.clone();
                let workspace_memory = AgentMemory::new(&self.context_builder.workspace());
                let mut session_clone = session.clone();
                let key_clone = session_key.clone();

                tokio::spawn(async move {
                    workspace_memory.consolidate(&mut session_clone, &providers, memory_window, false).await;
                    let _ = session_store.save(&session_clone).await;
                    consolidating_set.lock().await.remove(&key_clone);
                });
            }
        }

        let system_prompt = self.context_builder.build_system_prompt().await;
        let runtime_ctx = self.context_builder.build_runtime_context(&channel, &chat_id);
        let history = session.get_history(self.memory_window);

        let mut messages: Vec<LLMMessage> = Vec::new();
        messages.push(LLMMessage::system(system_prompt));
        for m in &history {
            messages.push(m.clone());
        }

        let user_content = if let Some(ref media) = media_path {
            format!("{}\n\n{}\n[media: {}]", runtime_ctx, content, media)
        } else {
            format!("{}\n\n{}", runtime_ctx, content)
        };

        let user_msg = if let Some((b64, mime)) = image_b64 {
            LLMMessage::user_with_image(user_content, b64, mime)
        } else {
            LLMMessage::user(user_content)
        };
        messages.push(user_msg);

        let initial_len = messages.len();

        let progress_cb: Option<ProgressCallback> = if self.send_progress {
            let bus = self.bus.clone();
            let ch = channel.clone();
            let cid = chat_id.clone();
            Some(Box::new(move |text: String, is_hint: bool| {
                let bus = bus.clone();
                let ch = ch.clone();
                let cid = cid.clone();
                Box::pin(async move {
                    bus.publish_outbound(OutboundMessage::progress(ch, cid, text, is_hint)).await;
                })
            }))
        } else {
            None
        };

        let file_cb: Option<FileDispatchCallback> = {
            let bus = self.bus.clone();
            let ch = channel.clone();
            let cid = chat_id.clone();
            let tracker = Arc::clone(&self.turn_file_tracker);
            Some(Box::new(move |path: std::path::PathBuf| {
                let bus = bus.clone();
                let ch = ch.clone();
                let cid = cid.clone();
                let tracker = Arc::clone(&tracker);
                Box::pin(async move {
                    if tracker.claim(&path).await {
                        bus.publish_outbound(OutboundMessage::file(ch, cid, path)).await;
                    }
                })
            }))
        };

        self.turn_file_tracker.reset().await;
        let registry = self.registry.lock().await;
        registry.set_turn_file_tracker(Arc::clone(&self.turn_file_tracker)).await;
        registry.reset_capture_turn_cache().await;
        let (result_content, final_messages, is_error) =
            run_loop(&self.providers, &*registry, messages, self.max_iterations, self.max_tokens, self.temperature, progress_cb.as_ref(), file_cb.as_ref()).await?;
        drop(registry);

        let reply = result_content.unwrap_or_else(|| "I was unable to produce a response.".to_string());

        if !is_error {
            session.messages.push(LLMMessage::user(content.clone()));
            let new_msgs = &final_messages[initial_len..];
            for m in new_msgs {
                if m.role == "assistant" {
                    let has_content = m.content.as_ref().map(|c| !c.is_empty()).unwrap_or(false);
                    let has_tool_calls = m.tool_calls.is_some();
                    if !has_content && !has_tool_calls {
                        continue;
                    }
                }
                let entry = m.clone();
                if m.role == "user" {
                    if let Some(ref c) = m.content {
                        if c.contains("[Runtime Context") {
                            continue;
                        }
                    }
                    if m.image_data.is_some() {
                        let mut stripped = entry.clone();
                        stripped.image_data = None;
                        stripped.content = Some("[image]".to_string());
                        session.messages.push(stripped);
                        continue;
                    }
                }
                session.messages.push(entry);
            }
            session.updated_at = chrono::Utc::now();
            let _ = self.session_store.save(&session).await;
        }

        let user_msg_event = shared::types::Message {
            id: uuid::Uuid::new_v4(),
            role: shared::types::MessageRole::User,
            content: content.clone(),
            timestamp: chrono::Utc::now(),
            media_path,
        };
        let _ = self.event_bus.publish(shared::events::PantherEvent::MessageReceived(user_msg_event));

        Ok(Some(OutboundMessage::text(channel, chat_id, reply)))
    }

    pub async fn new_session(&self, channel: &str, chat_id: &str) -> PantherResult<()> {
        let session_key = format!("{}:{}", channel, chat_id);
        self.stop(&session_key).await;

        let mut session = self.session_store.get_or_create(&session_key).await;
        if !session.messages.is_empty() {
            let succeeded = self.memory.consolidate(
                &mut session,
                &self.providers,
                self.memory_window,
                true,
            ).await;
            if !succeeded {
                return Err(PantherError::MemoryError(
                    "Memory archival failed — session not cleared. Try again.".to_string()
                ));
            }
        }
        session.clear();
        self.session_store.save_and_invalidate(&session).await
    }

    pub async fn dispatch_direct(
        &self,
        session_key: String,
        channel: String,
        chat_id: String,
        content: String,
        media_path: Option<String>,
        image_b64: Option<(String, String)>,
    ) -> PantherResult<String> {
        let msg = InboundMessage {
            channel,
            sender_id: "system".to_string(),
            chat_id,
            content,
            media_path,
            image_b64,
            session_key_override: Some(session_key),
        };
        let out = self.process_message(msg).await?;
        Ok(out.map(|o| o.content).unwrap_or_default())
    }
}
