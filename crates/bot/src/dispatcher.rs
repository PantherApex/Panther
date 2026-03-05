use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use shared::bus::{MessageBus, OutboundMessage};
use shared::channel::{Channel, FileKind};
use tokio::sync::Mutex;

struct StatusTracker {
    active: Mutex<HashMap<String, u64>>,
}

impl StatusTracker {
    fn new() -> Self {
        Self {
            active: Mutex::new(HashMap::new()),
        }
    }

    async fn get(&self, key: &str) -> Option<u64> {
        self.active.lock().await.get(key).copied()
    }

    async fn set(&self, key: String, message_id: u64) {
        self.active.lock().await.insert(key, message_id);
    }

    async fn remove(&self, key: &str) -> Option<u64> {
        self.active.lock().await.remove(key)
    }
}

pub struct OutboundDispatcher {
    bus: MessageBus,
    channels: Arc<HashMap<String, Arc<dyn Channel>>>,
    send_progress: bool,
    send_tool_hints: bool,
    telegram_token: String,
}

impl OutboundDispatcher {
    pub fn new(
        bus: MessageBus,
        channels: HashMap<String, Arc<dyn Channel>>,
        send_progress: bool,
        send_tool_hints: bool,
        telegram_token: String,
    ) -> Self {
        Self {
            bus,
            channels: Arc::new(channels),
            send_progress,
            send_tool_hints,
            telegram_token,
        }
    }

    pub async fn run(self) {
        let tracker = Arc::new(StatusTracker::new());

        while let Some(msg) = self.bus.consume_outbound().await {
            let is_cli = msg.channel == "cli";

            if msg.is_progress && !self.send_progress && !is_cli {
                continue;
            }
            if msg.is_tool_hint && !self.send_tool_hints && !is_cli {
                continue;
            }

            if msg.is_progress && !is_cli {
                self.handle_progress(&tracker, msg).await;
            } else {
                self.handle_final(&tracker, msg).await;
            }
        }
    }

    async fn handle_progress(&self, tracker: &StatusTracker, msg: OutboundMessage) {
        let status_key = format!("{}:{}", msg.channel, msg.chat_id);

        let channel = match self.channels.get(msg.channel.as_str()) {
            Some(ch) => Arc::clone(ch),
            None => {
                eprintln!("[panther:dispatcher] no channel registered for '{}'", msg.channel);
                return;
            }
        };

        if channel.supports_live_status() {
            match tracker.get(&status_key).await {
                None => {
                    match channel.send_status(&msg.chat_id, &msg.content).await {
                        Ok(Some(id)) => tracker.set(status_key, id).await,
                        Ok(None) => {}
                        Err(e) => eprintln!("[panther:dispatcher] send_status failed: {}", e),
                    }
                }
                Some(existing_id) => {
                    if let Err(e) = channel.edit_status(&msg.chat_id, existing_id, &msg.content).await {
                        eprintln!("[panther:dispatcher] edit_status failed: {}", e);
                    }
                }
            }
        } else {
            if let Err(e) = channel.send(&msg.chat_id, &msg.content).await {
                eprintln!("[panther:dispatcher] progress send to {} failed: {}", msg.channel, e);
            }
        }
    }

    async fn handle_final(&self, tracker: &StatusTracker, msg: OutboundMessage) {
        let is_cli = msg.channel == "cli";

        if !is_cli && msg.file_path.is_none() {
            let status_key = format!("{}:{}", msg.channel, msg.chat_id);
            if let Some(id) = tracker.remove(&status_key).await {
                if let Some(ch) = self.channels.get(msg.channel.as_str()) {
                    if ch.supports_live_status() {
                        if let Err(e) = ch.delete_status(&msg.chat_id, id).await {
                            eprintln!("[panther:dispatcher] delete_status failed: {}", e);
                        }
                    }
                }
            }
        }

        if msg.file_path.is_some() {
            let channels = Arc::clone(&self.channels);
            let token = self.telegram_token.clone();
            tokio::spawn(async move {
                dispatch_one(msg, channels, token).await;
            });
        } else {
            dispatch_one(msg, Arc::clone(&self.channels), self.telegram_token.clone()).await;
        }
    }
}

async fn dispatch_one(
    msg: OutboundMessage,
    channels: Arc<HashMap<String, Arc<dyn Channel>>>,
    telegram_token: String,
) {
    if let Some(path) = msg.file_path {
        dispatch_file(&msg.channel, &msg.chat_id, path, channels, telegram_token).await;
        return;
    }

    let (clean_text, embedded_files) = extract_panther_files(&msg.content);
    let clean_text = normalize_output(&clean_text);

    for file_path in embedded_files {
        let path = PathBuf::from(&file_path);
        if path.exists() {
            dispatch_file(&msg.channel, &msg.chat_id, path, Arc::clone(&channels), telegram_token.clone()).await;
        }
    }

    if clean_text.trim().is_empty() {
        return;
    }

    let channel_name = msg.channel.as_str();
    match channels.get(channel_name) {
        Some(ch) => {
            if let Err(e) = ch.send(&msg.chat_id, &clean_text).await {
                eprintln!("[panther:dispatcher] send to {} failed: {}", channel_name, e);
            }
        }
        None => {
            eprintln!("[panther:dispatcher] no channel registered for '{}'", channel_name);
        }
    }
}

async fn dispatch_file(
    channel_name: &str,
    chat_id: &str,
    path: PathBuf,
    channels: Arc<HashMap<String, Arc<dyn Channel>>>,
    _telegram_token: String,
) {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    let kind = if ["mp4", "avi", "mov", "mkv", "webm"].contains(&ext.as_str()) {
        FileKind::Video
    } else if ["png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff"].contains(&ext.as_str()) {
        FileKind::Photo
    } else if ["mp3", "ogg", "flac", "wav", "aac"].contains(&ext.as_str()) {
        FileKind::Audio
    } else {
        FileKind::Document
    };

    match channels.get(channel_name) {
        Some(ch) => {
            if let Err(e) = ch.send_file(chat_id, &path, kind).await {
                eprintln!("[panther:dispatcher] file send to {} failed: {}", channel_name, e);
            }
        }
        None => {
            eprintln!("[panther:dispatcher] no channel for file dispatch '{}'", channel_name);
        }
    }
}

fn extract_panther_files(response: &str) -> (String, Vec<String>) {
    let mut file_paths = Vec::new();
    let mut clean = response.to_string();
    loop {
        match clean.find("[PANTHER_FILE:") {
            Some(start) => match clean[start..].find(']') {
                Some(end) => {
                    let file_path = clean[start + 14..start + end].to_string();
                    file_paths.push(file_path);
                    clean = format!("{}{}", &clean[..start], &clean[start + end + 1..]);
                }
                None => break,
            },
            None => break,
        }
    }
    (clean.trim().to_string(), file_paths)
}

fn normalize_output(text: &str) -> String {
    text.replace("`n", "\n")
}
