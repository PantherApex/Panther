use std::pin::Pin;
use std::future::Future;
use std::sync::{Arc, RwLock};
use serde_json::json;
use shared::bus::{MessageBus, OutboundMessage};

use super::Tool;

pub struct MessageTool {
    bus: MessageBus,
    context: Arc<RwLock<MessageContext>>,
}

#[derive(Default, Clone)]
struct MessageContext {
    channel: String,
    chat_id: String,
}

impl MessageTool {
    pub fn new(bus: MessageBus) -> Self {
        Self {
            bus,
            context: Arc::new(RwLock::new(MessageContext::default())),
        }
    }

    pub fn set_context(&self, channel: String, chat_id: String) {
        if let Ok(mut ctx) = self.context.write() {
            ctx.channel = channel;
            ctx.chat_id = chat_id;
        }
    }
}

impl Tool for MessageTool {
    fn name(&self) -> &str { "message" }

    fn description(&self) -> &str {
        "Send a message to the current chat mid-task. Use for progress updates, intermediate results, or notifying the user without ending the task."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "The message text to send"
                },
                "chat_id": {
                    "type": "string",
                    "description": "Optional: override the destination chat_id (defaults to the current conversation)"
                }
            },
            "required": ["text"]
        })
    }

    fn execute<'a>(&'a self, args: serde_json::Value) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
        let bus = self.bus.clone();
        let context = Arc::clone(&self.context);
        Box::pin(async move {
            let text = match args.get("text").and_then(|v| v.as_str()) {
                Some(t) => t.to_string(),
                None => return "Error: missing 'text' argument".to_string(),
            };

            let ctx = context.read().map(|c| c.clone()).unwrap_or_default();

            let chat_id = args.get("chat_id")
                .and_then(|v| v.as_str())
                .unwrap_or(&ctx.chat_id)
                .to_string();

            if ctx.channel.is_empty() {
                return "Error: no channel context set".to_string();
            }
            if chat_id.is_empty() {
                return "Error: no chat_id available".to_string();
            }

            bus.publish_outbound(OutboundMessage::text(ctx.channel.clone(), chat_id.clone(), text)).await;
            format!("Message sent to {} chat {}", ctx.channel, chat_id)
        })
    }
}
