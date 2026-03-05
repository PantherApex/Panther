use std::collections::HashSet;
use std::path::PathBuf;
use std::pin::Pin;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use shared::bus::{InboundMessage, MessageBus};
use shared::channel::{Channel, FileKind};
use shared::errors::{PantherError, PantherResult};
use tokio::sync::RwLock;

pub const MATRIX_CHANNEL: &str = "matrix";

#[derive(Clone)]
pub struct MatrixConfig {
    pub homeserver: String,
    pub access_token: String,
    pub user_id: String,
    pub allow_from: Vec<String>,
    pub group_policy: MatrixGroupPolicy,
}

#[derive(Clone, PartialEq)]
pub enum MatrixGroupPolicy {
    Open,
    MentionOnly,
    AllowList,
}

#[derive(Deserialize)]
struct SyncResponse {
    next_batch: String,
    rooms: Option<SyncRooms>,
}

#[derive(Deserialize)]
struct SyncRooms {
    join: Option<std::collections::HashMap<String, JoinedRoom>>,
    invite: Option<std::collections::HashMap<String, Value>>,
}

#[derive(Deserialize)]
struct JoinedRoom {
    timeline: Option<Timeline>,
}

#[derive(Deserialize)]
struct Timeline {
    events: Vec<TimelineEvent>,
}

#[derive(Deserialize)]
struct TimelineEvent {
    event_id: Option<String>,
    #[serde(rename = "type")]
    event_type: String,
    sender: String,
    content: Value,
}

#[derive(Serialize)]
struct SendMessageBody {
    msgtype: String,
    body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    formatted_body: Option<String>,
}

pub struct MatrixChannel {
    config: MatrixConfig,
    allow_set: Arc<HashSet<String>>,
    http: Arc<Client>,
    bus: MessageBus,
    next_batch: Arc<RwLock<Option<String>>>,
    txn_counter: Arc<tokio::sync::Mutex<u64>>,
    seen_events: Arc<tokio::sync::Mutex<HashSet<String>>>,
}

impl MatrixChannel {
    pub fn new(config: MatrixConfig, bus: MessageBus) -> Self {
        let allow_set = Arc::new(config.allow_from.iter().cloned().collect::<HashSet<_>>());
        Self {
            config,
            allow_set,
            http: Arc::new(Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("Failed to build HTTP client for Matrix")),
            bus,
            next_batch: Arc::new(RwLock::new(None)),
            txn_counter: Arc::new(tokio::sync::Mutex::new(0)),
            seen_events: Arc::new(tokio::sync::Mutex::new(HashSet::new())),
        }
    }

    fn homeserver_url(&self, path: &str) -> String {
        format!("{}/_matrix/client/v3{}", self.config.homeserver.trim_end_matches('/'), path)
    }

    fn is_allowed_dm(&self, sender: &str) -> bool {
        if self.allow_set.is_empty() {
            return true;
        }
        self.allow_set.contains(sender)
    }

    fn is_allowed_room(&self, sender: &str, body: &str) -> bool {
        match self.config.group_policy {
            MatrixGroupPolicy::Open => true,
            MatrixGroupPolicy::MentionOnly => {
                let local = self.config.user_id
                    .split(':')
                    .next()
                    .unwrap_or("")
                    .trim_start_matches('@');
                body.contains(&self.config.user_id) || body.contains(local)
            }
            MatrixGroupPolicy::AllowList => self.allow_set.contains(sender),
        }
    }

    async fn get_room_members(&self, room_id: &str) -> usize {
        let url = self.homeserver_url(&format!("/rooms/{}/joined_members", urlencoded(room_id)));
        match self.http
            .get(&url)
            .bearer_auth(&self.config.access_token)
            .send()
            .await
        {
            Ok(resp) => {
                if let Ok(data) = resp.json::<Value>().await {
                    data["joined"].as_object().map(|m| m.len()).unwrap_or(2)
                } else {
                    2
                }
            }
            Err(_) => 2,
        }
    }

    async fn auto_join_pending_invites(&self) {
        let url = self.homeserver_url("/sync?timeout=0&filter=%7B%22room%22%3A%7B%22timeline%22%3A%7B%22limit%22%3A0%7D%7D%7D");
        let since = self.next_batch.read().await.clone();
        let url_with_since = since.as_ref()
            .map(|s| format!("{}&since={}", url, s))
            .unwrap_or(url);

        if let Ok(resp) = self.http
            .get(&url_with_since)
            .bearer_auth(&self.config.access_token)
            .send()
            .await
        {
            if let Ok(sync) = resp.json::<SyncResponse>().await {
                if let Some(rooms) = sync.rooms {
                    if let Some(invites) = rooms.invite {
                        for room_id in invites.keys() {
                            self.join_room(room_id).await;
                        }
                    }
                }
            }
        }
    }

    async fn join_room(&self, room_id: &str) {
        let url = self.homeserver_url(&format!("/join/{}", urlencoded(room_id)));
        let _ = self.http
            .post(&url)
            .bearer_auth(&self.config.access_token)
            .json(&json!({}))
            .send()
            .await;
        eprintln!("[panther:matrix] joined room {}", room_id);
    }

    async fn next_txn_id(&self) -> String {
        let mut counter = self.txn_counter.lock().await;
        *counter += 1;
        format!("panther-{}-{}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(), *counter)
    }

    pub async fn run_loop(self: Arc<Self>) {
        self.resolve_initial_batch().await;
        self.auto_join_pending_invites().await;

        let mut backoff = Duration::from_secs(1);

        loop {
            match self.sync_once().await {
                Ok(_) => {
                    backoff = Duration::from_secs(1);
                }
                Err(e) => {
                    eprintln!("[panther:matrix] sync error: {} — retrying in {:?}", e, backoff);
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(60));
                }
            }
        }
    }

    async fn resolve_initial_batch(&self) {
        let url = self.homeserver_url("/sync?timeout=0&filter=%7B%22room%22%3A%7B%22timeline%22%3A%7B%22limit%22%3A0%7D%7D%7D");
        match self.http
            .get(&url)
            .bearer_auth(&self.config.access_token)
            .send()
            .await
        {
            Ok(resp) => {
                if let Ok(sync) = resp.json::<SyncResponse>().await {
                    *self.next_batch.write().await = Some(sync.next_batch);
                }
            }
            Err(e) => eprintln!("[panther:matrix] initial sync failed: {}", e),
        }
    }

    async fn sync_once(&self) -> PantherResult<()> {
        let since = self.next_batch.read().await.clone();
        let url = match &since {
            Some(s) => format!("{}?timeout=30000&since={}", self.homeserver_url("/sync"), s),
            None => format!("{}?timeout=30000", self.homeserver_url("/sync")),
        };

        let resp = self.http
            .get(&url)
            .bearer_auth(&self.config.access_token)
            .timeout(Duration::from_secs(40))
            .send()
            .await
            .map_err(|e| PantherError::ChannelError(format!("Matrix sync failed: {}", e)))?;

        if resp.status() == StatusCode::UNAUTHORIZED {
            return Err(PantherError::ChannelError("Matrix access token invalid or expired".to_string()));
        }

        let sync: SyncResponse = resp.json().await
            .map_err(|e| PantherError::ChannelError(format!("Matrix sync parse: {}", e)))?;

        *self.next_batch.write().await = Some(sync.next_batch);

        if let Some(rooms) = sync.rooms {
            if let Some(invites) = rooms.invite {
                for room_id in invites.keys() {
                    self.join_room(room_id).await;
                }
            }

            if let Some(joined) = rooms.join {
                for (room_id, room_data) in &joined {
                    if let Some(timeline) = &room_data.timeline {
                        for event in &timeline.events {
                            self.process_event(room_id, event).await;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    async fn process_event(&self, room_id: &str, event: &TimelineEvent) {
        if let Some(id) = &event.event_id {
            let mut seen = self.seen_events.lock().await;
            if !seen.insert(id.clone()) {
                return;
            }
            if seen.len() > 10_000 {
                seen.clear();
            }
        }

        if event.event_type != "m.room.message" {
            return;
        }

        if event.sender == self.config.user_id {
            return;
        }

        let msg_type = event.content.get("msgtype").and_then(|v| v.as_str()).unwrap_or("");
        if msg_type != "m.text" && msg_type != "m.image" && msg_type != "m.file" && msg_type != "m.audio" {
            return;
        }

        let body = event.content.get("body").and_then(|v| v.as_str()).unwrap_or("").to_string();

        let member_count = self.get_room_members(room_id).await;
        let is_dm = member_count <= 2;

        if is_dm {
            if !self.is_allowed_dm(&event.sender) {
                return;
            }
        } else if !self.is_allowed_room(&event.sender, &body) {
            return;
        }

        let (content, media_path) = if msg_type == "m.text" {
            (body, None)
        } else {
            let mxc_url = event.content.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let path = if !mxc_url.is_empty() {
                self.download_mxc(&mxc_url, &body).await
                    .map(|p| p.to_string_lossy().to_string())
            } else {
                None
            };
            (format!("[{}]", msg_type.trim_start_matches("m.")), path)
        };

        let inbound = InboundMessage {
            channel: MATRIX_CHANNEL.to_string(),
            sender_id: event.sender.clone(),
            chat_id: room_id.to_string(),
            content,
            media_path,
            image_b64: None,
            session_key_override: None,
        };

        self.bus.publish_inbound(inbound).await;
    }

    async fn download_mxc(&self, mxc_url: &str, filename: &str) -> Option<PathBuf> {
        let stripped = mxc_url.strip_prefix("mxc://")?;
        let http_url = format!("{}/_matrix/media/v3/download/{}", self.config.homeserver.trim_end_matches('/'), stripped);

        let temp_dir = dirs::home_dir()?.join(".panther").join("temp");
        let _ = tokio::fs::create_dir_all(&temp_dir).await;

        let safe_name = filename.chars().filter(|c| c.is_alphanumeric() || *c == '.' || *c == '_').collect::<String>();
        let dest = temp_dir.join(format!("matrix_{}", safe_name));

        match self.http.get(&http_url).bearer_auth(&self.config.access_token).send().await {
            Ok(resp) if resp.status().is_success() => {
                if let Ok(bytes) = resp.bytes().await {
                    if tokio::fs::write(&dest, &bytes).await.is_ok() {
                        return Some(dest);
                    }
                }
            }
            _ => {}
        }
        None
    }

    async fn upload_media(&self, bytes: Vec<u8>, filename: &str, mime: &str) -> PantherResult<String> {
        let url = format!(
            "{}/_matrix/media/v3/upload?filename={}",
            self.config.homeserver.trim_end_matches('/'),
            urlencoded(filename)
        );

        let resp = self.http
            .post(&url)
            .bearer_auth(&self.config.access_token)
            .header("Content-Type", mime)
            .body(bytes)
            .send()
            .await
            .map_err(|e| PantherError::ChannelError(format!("Matrix upload failed: {}", e)))?;

        let data: Value = resp.json().await
            .map_err(|e| PantherError::ChannelError(format!("Matrix upload parse: {}", e)))?;

        data["content_uri"].as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| PantherError::ChannelError("Matrix upload: missing content_uri".to_string()))
    }
}

fn urlencoded(s: &str) -> String {
    s.chars().map(|c| match c {
        'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
        _ => format!("%{:02X}", c as u32),
    }).collect()
}

impl Channel for MatrixChannel {
    fn name(&self) -> &str {
        MATRIX_CHANNEL
    }

    fn send<'a>(
        &'a self,
        chat_id: &'a str,
        text: &'a str,
    ) -> Pin<Box<dyn Future<Output = PantherResult<()>> + Send + 'a>> {
        Box::pin(async move {
            const MATRIX_LIMIT: usize = 32768;
            let chars: Vec<char> = text.chars().collect();

            for chunk in chars.chunks(MATRIX_LIMIT) {
                let part: String = chunk.iter().collect();
                let txn_id = self.next_txn_id().await;
                let url = self.homeserver_url(&format!(
                    "/rooms/{}/send/m.room.message/{}",
                    urlencoded(chat_id),
                    txn_id
                ));

                let body = SendMessageBody {
                    msgtype: "m.text".to_string(),
                    body: part.clone(),
                    format: Some("org.matrix.custom.html".to_string()),
                    formatted_body: Some(markdown_to_html(&part)),
                };

                let resp = self.http
                    .put(&url)
                    .bearer_auth(&self.config.access_token)
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| PantherError::ChannelError(format!("Matrix send failed: {}", e)))?;

                if !resp.status().is_success() {
                    let status = resp.status();
                    let err: Value = resp.json().await.unwrap_or_default();
                    return Err(PantherError::ChannelError(format!(
                        "Matrix API error {}: {}",
                        status,
                        err.get("error").and_then(|v| v.as_str()).unwrap_or("unknown")
                    )));
                }
            }

            Ok(())
        })
    }

    fn send_file<'a>(
        &'a self,
        chat_id: &'a str,
        path: &'a PathBuf,
        kind: FileKind,
    ) -> Pin<Box<dyn Future<Output = PantherResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let bytes = tokio::fs::read(path).await
                .map_err(|e| PantherError::ChannelError(format!("Failed to read file: {}", e)))?;

            let filename = path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "file".to_string());

            let (mime, msgtype) = match kind {
                FileKind::Photo => ("image/jpeg", "m.image"),
                FileKind::Video => ("video/mp4", "m.video"),
                FileKind::Audio => ("audio/ogg", "m.audio"),
                FileKind::Document => ("application/octet-stream", "m.file"),
            };

            let content_uri = self.upload_media(bytes, &filename, mime).await?;
            let txn_id = self.next_txn_id().await;

            let url = self.homeserver_url(&format!(
                "/rooms/{}/send/m.room.message/{}",
                urlencoded(chat_id),
                txn_id
            ));

            let body = json!({
                "msgtype": msgtype,
                "body": filename,
                "url": content_uri
            });

            let resp = self.http
                .put(&url)
                .bearer_auth(&self.config.access_token)
                .json(&body)
                .send()
                .await
                .map_err(|e| PantherError::ChannelError(format!("Matrix file send failed: {}", e)))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let err: Value = resp.json().await.unwrap_or_default();
                return Err(PantherError::ChannelError(format!(
                    "Matrix file API error {}: {}",
                    status,
                    err.get("error").and_then(|v| v.as_str()).unwrap_or("unknown")
                )));
            }

            Ok(())
        })
    }
}

fn markdown_to_html(text: &str) -> String {
    let mut html = String::new();
    for line in text.lines() {
        let escaped = line
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");

        let formatted = format_inline_markdown(&escaped);
        html.push_str(&formatted);
        html.push_str("<br/>");
    }
    html.trim_end_matches("<br/>").to_string()
}

fn format_inline_markdown(s: &str) -> String {
    let mut result = s.to_string();
    result = replace_delimited(&result, "**", "<strong>", "</strong>");
    result = replace_delimited(&result, "__", "<strong>", "</strong>");
    result = replace_delimited(&result, "*", "<em>", "</em>");
    result = replace_delimited(&result, "_", "<em>", "</em>");
    result = replace_delimited(&result, "`", "<code>", "</code>");
    result
}

fn replace_delimited(s: &str, delim: &str, open: &str, close: &str) -> String {
    let mut result = String::new();
    let mut remaining = s;
    let mut open_tag = false;

    while let Some(pos) = remaining.find(delim) {
        result.push_str(&remaining[..pos]);
        if open_tag {
            result.push_str(close);
            open_tag = false;
        } else {
            result.push_str(open);
            open_tag = true;
        }
        remaining = &remaining[pos + delim.len()..];
    }

    if open_tag {
        result.push_str(delim);
        result.push_str(remaining);
    } else {
        result.push_str(remaining);
    }

    result
}
