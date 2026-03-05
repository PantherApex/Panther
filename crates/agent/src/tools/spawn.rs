use std::pin::Pin;
use std::future::Future;
use std::sync::{Arc, RwLock};
use serde_json::json;

use crate::subagent::SubagentManager;
use super::Tool;

struct SpawnContext {
    channel: String,
    chat_id: String,
    session_key: String,
}

pub struct SpawnTool {
    manager: Arc<SubagentManager>,
    context: Arc<RwLock<SpawnContext>>,
}

impl SpawnTool {
    pub fn new(manager: Arc<SubagentManager>) -> Self {
        Self {
            manager,
            context: Arc::new(RwLock::new(SpawnContext {
                channel: String::new(),
                chat_id: String::new(),
                session_key: String::new(),
            })),
        }
    }

    pub fn set_context(&self, channel: String, chat_id: String) {
        if let Ok(mut ctx) = self.context.write() {
            ctx.session_key = format!("{}:{}", channel, chat_id);
            ctx.channel = channel;
            ctx.chat_id = chat_id;
        }
    }
}

impl Tool for SpawnTool {
    fn name(&self) -> &str { "spawn" }

    fn description(&self) -> &str {
        "Spawn a subagent to handle a task in the background. Use for complex or long-running tasks that can run independently. The subagent has access to all tools except message and spawn. You will be notified when it completes."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "Full description of the task for the subagent to complete"
                },
                "label": {
                    "type": "string",
                    "description": "Short label for the task (optional, for display)"
                }
            },
            "required": ["task"]
        })
    }

    fn execute<'a>(&'a self, args: serde_json::Value) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
        let manager = Arc::clone(&self.manager);
        let context = Arc::clone(&self.context);
        Box::pin(async move {
            let task = match args.get("task").and_then(|v| v.as_str()) {
                Some(t) => t.to_string(),
                None => return "Error: missing 'task'".to_string(),
            };
            let label = args.get("label").and_then(|v| v.as_str()).map(|s| s.to_string());

            let (channel, chat_id, session_key) = {
                let ctx = context.read().unwrap();
                (ctx.channel.clone(), ctx.chat_id.clone(), ctx.session_key.clone())
            };

            if channel.is_empty() {
                return "Error: no session context available".to_string();
            }

            manager.spawn(task, label, channel, chat_id, session_key).await
        })
    }
}
