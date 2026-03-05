use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use shared::bus::{InboundMessage, MessageBus};
use shared::channel::{Channel, FileKind};
use shared::errors::{PantherError, PantherResult};
use std::path::PathBuf;
use std::pin::Pin;
use std::future::Future;
use tokio::sync::RwLock;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;

pub const SLACK_CHANNEL: &str = "slack";

#[derive(Deserialize)]
struct WsOpenResponse {
    ok: bool,
    url: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct AuthTestResponse {
    ok: bool,
    user_id: Option<String>,
}

pub struct SlackChannel {
    app_token: String,
    bot_token: String,
    allow_from: Arc<HashSet<String>>,
    bot_user_id: Arc<RwLock<Option<String>>>,
    http: Arc<Client>,
    bus: MessageBus,
}

impl SlackChannel {
    pub fn new(app_token: String, bot_token: String, allow_from: Vec<String>, bus: MessageBus) -> Self {
        Self {
            app_token,
            bot_token,
            allow_from: Arc::new(allow_from.into_iter().collect()),
            bot_user_id: Arc::new(RwLock::new(None)),
            http: Arc::new(Client::new()),
            bus,
        }
    }

    fn is_allowed(&self, user_id: &str) -> bool {
        self.allow_from.is_empty() || self.allow_from.contains(user_id)
    }

    async fn resolve_bot_user_id(&self) {
        match self.http
            .post("https://slack.com/api/auth.test")
            .bearer_auth(&self.bot_token)
            .send()
            .await
        {
            Ok(resp) => {
                if let Ok(data) = resp.json::<AuthTestResponse>().await {
                    if data.ok {
                        if let Some(uid) = data.user_id {
                            *self.bot_user_id.write().await = Some(uid);
                        }
                    }
                }
            }
            Err(e) => eprintln!("[panther:slack] auth.test failed: {}", e),
        }
    }

    async fn open_wss_url(&self) -> PantherResult<String> {
        let resp = self.http
            .post("https://slack.com/api/apps.connections.open")
            .bearer_auth(&self.app_token)
            .send()
            .await
            .map_err(|e| PantherError::ChannelError(format!("Slack WS open failed: {}", e)))?;

        let data: WsOpenResponse = resp.json().await
            .map_err(|e| PantherError::ChannelError(format!("Slack WS open parse failed: {}", e)))?;

        if !data.ok {
            return Err(PantherError::ChannelError(format!(
                "Slack apps.connections.open error: {}",
                data.error.unwrap_or_else(|| "unknown".to_string())
            )));
        }

        data.url.ok_or_else(|| PantherError::ChannelError("Slack WS URL missing".to_string()))
    }

    async fn ack_envelope(
        sink: &mut (impl SinkExt<WsMessage, Error = tokio_tungstenite::tungstenite::Error> + Unpin),
        envelope_id: &str,
    ) {
        let ack = json!({ "envelope_id": envelope_id });
        let _ = sink.send(WsMessage::Text(ack.to_string())).await;
    }

    async fn handle_payload(&self, payload: &Value) {
        let event_type = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");

        if event_type != "event_callback" {
            return;
        }

        let event = match payload.get("event") {
            Some(e) => e,
            None => return,
        };

        let sub_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if sub_type != "message" {
            return;
        }

        if event.get("subtype").is_some() {
            return;
        }

        let user_id = match event.get("user").and_then(|v| v.as_str()) {
            Some(u) => u.to_string(),
            None => return,
        };

        let bot_uid = self.bot_user_id.read().await.clone();
        if Some(user_id.as_str()) == bot_uid.as_deref() {
            return;
        }

        if !self.is_allowed(&user_id) {
            return;
        }

        let text = event.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let channel_id = match event.get("channel").and_then(|v| v.as_str()) {
            Some(c) => c.to_string(),
            None => return,
        };
        let thread_ts = event.get("thread_ts").and_then(|v| v.as_str()).map(|s| s.to_string());

        let session_key = thread_ts.as_ref().map(|ts| {
            format!("{}:{}:{}", SLACK_CHANNEL, channel_id, ts)
        });

        let files = self.extract_files(event).await;

        let inbound = InboundMessage {
            channel: SLACK_CHANNEL.to_string(),
            sender_id: user_id,
            chat_id: channel_id,
            content: if text.is_empty() && !files.is_empty() {
                "[file attached]".to_string()
            } else {
                text
            },
            media_path: files.first().cloned(),
            image_b64: None,
            session_key_override: session_key,
        };

        self.bus.publish_inbound(inbound).await;
    }

    async fn extract_files(&self, event: &Value) -> Vec<String> {
        let files_arr = match event.get("files").and_then(|v| v.as_array()) {
            Some(a) => a.clone(),
            None => return Vec::new(),
        };

        let mut paths = Vec::new();
        let temp_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(".panther")
            .join("temp");

        let _ = tokio::fs::create_dir_all(&temp_dir).await;

        for file in &files_arr {
            let url = match file.get("url_private_download").and_then(|v| v.as_str()) {
                Some(u) => u.to_string(),
                None => continue,
            };
            let name = file.get("name").and_then(|v| v.as_str()).unwrap_or("file");
            let id = file.get("id").and_then(|v| v.as_str()).unwrap_or("unknown");
            let dest = temp_dir.join(format!("{}_{}", id, name));

            match self.http.get(&url).bearer_auth(&self.bot_token).send().await {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(bytes) = resp.bytes().await {
                        if tokio::fs::write(&dest, &bytes).await.is_ok() {
                            paths.push(dest.to_string_lossy().to_string());
                        }
                    }
                }
                _ => {}
            }
        }

        paths
    }

    pub async fn run_loop(self: Arc<Self>) {
        let mut backoff = Duration::from_secs(1);

        loop {
            match self.run_once().await {
                Ok(()) => {
                    backoff = Duration::from_secs(1);
                }
                Err(e) => {
                    eprintln!("[panther:slack] connection error: {} — reconnecting in {:?}", e, backoff);
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(60));
                }
            }
        }
    }

    async fn run_once(&self) -> PantherResult<()> {
        self.resolve_bot_user_id().await;
        let url = self.open_wss_url().await?;

        let (ws_stream, _) = connect_async(&url).await
            .map_err(|e| PantherError::ChannelError(format!("Slack WS connect failed: {}", e)))?;

        eprintln!("[panther:slack] Socket Mode connected");

        let (mut sink, mut stream) = ws_stream.split();

        while let Some(msg) = stream.next().await {
            let msg = msg.map_err(|e| PantherError::ChannelError(format!("Slack WS recv: {}", e)))?;

            match msg {
                WsMessage::Text(text) => {
                    let val: Value = match serde_json::from_str(&text) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                    let msg_type = val.get("type").and_then(|v| v.as_str()).unwrap_or("");

                    if msg_type == "hello" {
                        eprintln!("[panther:slack] Socket Mode handshake complete");
                        continue;
                    }

                    if msg_type == "disconnect" {
                        let reason = val.get("reason").and_then(|v| v.as_str()).unwrap_or("unknown");
                        return Err(PantherError::ChannelError(format!("Slack requested disconnect: {}", reason)));
                    }

                    if let Some(envelope_id) = val.get("envelope_id").and_then(|v| v.as_str()) {
                        SlackChannel::ack_envelope(&mut sink, envelope_id).await;
                    }

                    if let Some(payload) = val.get("payload") {
                        self.handle_payload(payload).await;
                    }
                }
                WsMessage::Ping(data) => {
                    let _ = sink.send(WsMessage::Pong(data)).await;
                }
                WsMessage::Close(_) => {
                    return Err(PantherError::ChannelError("Slack WS closed".to_string()));
                }
                _ => {}
            }
        }

        Err(PantherError::ChannelError("Slack WS stream ended".to_string()))
    }
}

impl Channel for SlackChannel {
    fn name(&self) -> &str {
        SLACK_CHANNEL
    }

    fn send<'a>(
        &'a self,
        chat_id: &'a str,
        text: &'a str,
    ) -> Pin<Box<dyn Future<Output = PantherResult<()>> + Send + 'a>> {
        Box::pin(async move {
            const SLACK_LIMIT: usize = 3000;
            let chars: Vec<char> = text.chars().collect();

            for chunk in chars.chunks(SLACK_LIMIT) {
                let part: String = chunk.iter().collect();

                let body = json!({
                    "channel": chat_id,
                    "text": part
                });

                let resp = self.http
                    .post("https://slack.com/api/chat.postMessage")
                    .bearer_auth(&self.bot_token)
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| PantherError::ChannelError(format!("Slack send failed: {}", e)))?;

                let data: Value = resp.json().await
                    .map_err(|e| PantherError::ChannelError(format!("Slack send parse: {}", e)))?;

                if !data.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                    let err = data.get("error").and_then(|v| v.as_str()).unwrap_or("unknown");
                    return Err(PantherError::ChannelError(format!("Slack API error: {}", err)));
                }
            }

            Ok(())
        })
    }

    fn send_file<'a>(
        &'a self,
        chat_id: &'a str,
        path: &'a PathBuf,
        _kind: FileKind,
    ) -> Pin<Box<dyn Future<Output = PantherResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let bytes = tokio::fs::read(path).await
                .map_err(|e| PantherError::ChannelError(format!("Failed to read file: {}", e)))?;

            let filename = path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "file".to_string());

            let get_url_body = json!({
                "channels": chat_id,
                "filename": filename,
                "length": bytes.len()
            });

            let url_resp: Value = self.http
                .post("https://slack.com/api/files.getUploadURLExternal")
                .bearer_auth(&self.bot_token)
                .json(&get_url_body)
                .send()
                .await
                .map_err(|e| PantherError::ChannelError(format!("Slack upload URL fetch failed: {}", e)))?
                .json()
                .await
                .map_err(|e| PantherError::ChannelError(format!("Slack upload URL parse: {}", e)))?;

            if !url_resp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                let err = url_resp.get("error").and_then(|v| v.as_str()).unwrap_or("unknown");
                return Err(PantherError::ChannelError(format!("Slack getUploadURL error: {}", err)));
            }

            let upload_url = url_resp["upload_url"].as_str()
                .ok_or_else(|| PantherError::ChannelError("Missing upload_url".to_string()))?;
            let file_id = url_resp["file_id"].as_str()
                .ok_or_else(|| PantherError::ChannelError("Missing file_id".to_string()))?;

            let part = reqwest::multipart::Part::bytes(bytes).file_name(filename);
            let form = reqwest::multipart::Form::new().part("files", part);

            self.http
                .post(upload_url)
                .bearer_auth(&self.bot_token)
                .multipart(form)
                .send()
                .await
                .map_err(|e| PantherError::ChannelError(format!("Slack file upload failed: {}", e)))?;

            let complete_body = json!({
                "files": [{ "id": file_id }],
                "channel_id": chat_id
            });

            let complete_resp: Value = self.http
                .post("https://slack.com/api/files.completeUploadExternal")
                .bearer_auth(&self.bot_token)
                .json(&complete_body)
                .send()
                .await
                .map_err(|e| PantherError::ChannelError(format!("Slack file complete failed: {}", e)))?
                .json()
                .await
                .map_err(|e| PantherError::ChannelError(format!("Slack file complete parse: {}", e)))?;

            if !complete_resp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                let err = complete_resp.get("error").and_then(|v| v.as_str()).unwrap_or("unknown");
                return Err(PantherError::ChannelError(format!("Slack completeUpload error: {}", err)));
            }

            Ok(())
        })
    }
}
