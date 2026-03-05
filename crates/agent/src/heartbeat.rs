use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::task::JoinHandle;
use serde_json::{json, Value};

use providers::ProviderRouter;
use shared::types::{LLMMessage, LLMRequest, ToolDefinition};

pub type ExecuteFn = Arc<dyn Fn(String) -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send>> + Send + Sync>;
pub type NotifyFn = Arc<dyn Fn(String) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync>;

pub struct HeartbeatService {
    workspace: PathBuf,
    providers: ProviderRouter,
    on_execute: Option<ExecuteFn>,
    on_notify: Option<NotifyFn>,
    interval_secs: u64,
    running: Arc<AtomicBool>,
    task: Arc<tokio::sync::Mutex<Option<JoinHandle<()>>>>,
}

impl HeartbeatService {
    pub fn new(
        workspace: PathBuf,
        providers: ProviderRouter,
        interval_secs: u64,
    ) -> Self {
        Self {
            workspace,
            providers,
            on_execute: None,
            on_notify: None,
            interval_secs,
            running: Arc::new(AtomicBool::new(false)),
            task: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    pub fn with_execute(mut self, f: ExecuteFn) -> Self {
        self.on_execute = Some(f);
        self
    }

    pub fn with_notify(mut self, f: NotifyFn) -> Self {
        self.on_notify = Some(f);
        self
    }

    pub async fn start(self: Arc<Self>) {
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let svc = Arc::clone(&self);
        let handle = tokio::spawn(async move {
            svc.run_loop().await;
        });
        *self.task.lock().await = Some(handle);
    }

    pub async fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.task.lock().await.take() {
            handle.abort();
        }
    }

    async fn run_loop(&self) {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(self.interval_secs)).await;
            if !self.running.load(Ordering::SeqCst) {
                break;
            }
            self.tick().await;
        }
    }

    async fn tick(&self) {
        let content = match self.read_heartbeat_file().await {
            Some(c) if !c.trim().is_empty() => c,
            _ => return,
        };

        let (action, tasks) = self.decide(&content).await;
        if action != "run" {
            return;
        }

        if let Some(execute) = &self.on_execute {
            let result = (execute)(tasks).await;
            if !result.is_empty() {
                if let Some(notify) = &self.on_notify {
                    (notify)(result).await;
                }
            }
        }
    }

    async fn read_heartbeat_file(&self) -> Option<String> {
        let path = self.workspace.join("HEARTBEAT.md");
        tokio::fs::read_to_string(&path).await.ok()
    }

    async fn decide(&self, content: &str) -> (String, String) {
        let heartbeat_tool = ToolDefinition {
            name: "heartbeat".to_string(),
            description: "Report heartbeat decision after reviewing tasks.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["skip", "run"],
                        "description": "skip = nothing to do, run = has active tasks to execute"
                    },
                    "tasks": {
                        "type": "string",
                        "description": "Natural-language summary of active tasks (required for run)"
                    }
                },
                "required": ["action"]
            }),
        };

        let request = LLMRequest {
            model: self.providers.active_model(),
            messages: vec![
                LLMMessage::system("You are a heartbeat agent. Call the heartbeat tool to report your decision.".to_string()),
                LLMMessage::user(format!(
                    "Review the following HEARTBEAT.md and decide whether there are active tasks that need execution.\n\n{}",
                    content
                )),
            ],
            temperature: Some(0.1),
            max_tokens: Some(256),
            tools: Some(vec![heartbeat_tool]),
        };

        let response = match self.providers.chat(request).await {
            Ok(r) => r,
            Err(_) => return ("skip".to_string(), String::new()),
        };

        let tool_calls = match response.tool_calls {
            Some(calls) if !calls.is_empty() => calls,
            _ => return ("skip".to_string(), String::new()),
        };

        let args: Value = match &tool_calls[0].arguments {
            v if v.is_string() => serde_json::from_str(v.as_str().unwrap_or("{}")).unwrap_or_default(),
            v => v.clone(),
        };

        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("skip").to_string();
        let tasks = args.get("tasks").and_then(|v| v.as_str()).unwrap_or("").to_string();
        (action, tasks)
    }

    pub async fn trigger_now(&self) -> Option<String> {
        let content = self.read_heartbeat_file().await?;
        if content.trim().is_empty() { return None; }
        let (action, tasks) = self.decide(&content).await;
        if action != "run" { return None; }
        if let Some(execute) = &self.on_execute {
            return Some((execute)(tasks).await);
        }
        None
    }
}
