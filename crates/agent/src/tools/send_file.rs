use std::path::PathBuf;
use std::pin::Pin;
use std::future::Future;
use std::sync::{Arc, RwLock};
use tokio::sync::Mutex;
use serde_json::json;
use shared::bus::{MessageBus, OutboundMessage};

use crate::file_tracker::TurnFileTracker;
use super::Tool;

pub struct SendFileTool {
    bus: MessageBus,
    context: Arc<RwLock<FileContext>>,
    turn_tracker: Arc<Mutex<Option<Arc<TurnFileTracker>>>>,
}

#[derive(Default, Clone)]
struct FileContext {
    channel: String,
    chat_id: String,
}

impl SendFileTool {
    pub fn new(bus: MessageBus) -> Self {
        Self {
            bus,
            context: Arc::new(RwLock::new(FileContext::default())),
            turn_tracker: Arc::new(Mutex::new(None)),
        }
    }

    pub fn set_context(&self, channel: String, chat_id: String) {
        if let Ok(mut ctx) = self.context.write() {
            ctx.channel = channel;
            ctx.chat_id = chat_id;
        }
    }

    pub async fn set_turn_tracker(&self, tracker: Arc<TurnFileTracker>) {
        *self.turn_tracker.lock().await = Some(tracker);
    }
}

impl Tool for SendFileTool {
    fn name(&self) -> &str { "send_file" }

    fn description(&self) -> &str {
        "Send a file directly to the current chat. Supports images, audio, video, and documents. The file must exist on disk. Use this after capture_media or after creating/downloading any file you want to share with the user."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to the file to send"
                },
                "chat_id": {
                    "type": "string",
                    "description": "Optional: override destination chat_id (defaults to current conversation)"
                }
            },
            "required": ["path"]
        })
    }

    fn execute<'a>(&'a self, args: serde_json::Value) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
        let bus = self.bus.clone();
        let context = Arc::clone(&self.context);
        let turn_tracker = Arc::clone(&self.turn_tracker);
        Box::pin(async move {
            let raw_path = match args.get("path").and_then(|v| v.as_str()) {
                Some(p) => p.to_string(),
                None => return "Error: missing 'path' argument".to_string(),
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

            let normalized = raw_path.replace('\\', "/");
            let path = if normalized.starts_with("~/") || normalized == "~" {
                if let Some(home) = dirs::home_dir() {
                    let rest = normalized.trim_start_matches("~/").trim_start_matches('~');
                    if rest.is_empty() { home } else { home.join(rest) }
                } else {
                    PathBuf::from(&raw_path)
                }
            } else {
                PathBuf::from(&raw_path)
            };

            if !path.exists() {
                return format!("Error: file not found at {}", path.display());
            }

            let size = tokio::fs::metadata(&path).await
                .map(|m| m.len())
                .unwrap_or(0);

            if size == 0 {
                return format!("Error: file at {} is empty (0 bytes)", path.display());
            }

            let filename = path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "file".to_string());

            let tracker_guard = turn_tracker.lock().await;
            if let Some(ref tracker) = *tracker_guard {
                if !tracker.claim(&path).await {
                    return format!("File '{}' already delivered to chat this turn.", filename);
                }
            }
            drop(tracker_guard);

            bus.publish_outbound(OutboundMessage::file(ctx.channel.clone(), chat_id.clone(), path)).await;

            format!("File '{}' ({} bytes) sent to {} chat {}", filename, size, ctx.channel, chat_id)
        })
    }
}
