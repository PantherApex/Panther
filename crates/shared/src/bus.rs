use std::path::PathBuf;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct InboundMessage {
    pub channel: String,
    pub sender_id: String,
    pub chat_id: String,
    pub content: String,
    pub media_path: Option<String>,
    pub image_b64: Option<(String, String)>,
    pub session_key_override: Option<String>,
}

impl InboundMessage {
    pub fn session_key(&self) -> String {
        self.session_key_override
            .clone()
            .unwrap_or_else(|| format!("{}:{}", self.channel, self.chat_id))
    }
}

#[derive(Debug, Clone)]
pub struct OutboundMessage {
    pub channel: String,
    pub chat_id: String,
    pub content: String,
    pub file_path: Option<PathBuf>,
    pub is_progress: bool,
    pub is_tool_hint: bool,
}

impl OutboundMessage {
    pub fn text(channel: impl Into<String>, chat_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            channel: channel.into(),
            chat_id: chat_id.into(),
            content: content.into(),
            file_path: None,
            is_progress: false,
            is_tool_hint: false,
        }
    }

    pub fn progress(channel: impl Into<String>, chat_id: impl Into<String>, content: impl Into<String>, is_tool_hint: bool) -> Self {
        Self {
            channel: channel.into(),
            chat_id: chat_id.into(),
            content: content.into(),
            file_path: None,
            is_progress: true,
            is_tool_hint,
        }
    }

    pub fn file(channel: impl Into<String>, chat_id: impl Into<String>, path: PathBuf) -> Self {
        Self {
            channel: channel.into(),
            chat_id: chat_id.into(),
            content: String::new(),
            file_path: Some(path),
            is_progress: false,
            is_tool_hint: false,
        }
    }
}

#[derive(Clone)]
pub struct MessageBus {
    inbound_tx: mpsc::Sender<InboundMessage>,
    inbound_rx: std::sync::Arc<tokio::sync::Mutex<mpsc::Receiver<InboundMessage>>>,
    outbound_tx: mpsc::Sender<OutboundMessage>,
    outbound_rx: std::sync::Arc<tokio::sync::Mutex<mpsc::Receiver<OutboundMessage>>>,
}

impl MessageBus {
    pub fn new() -> Self {
        let (inbound_tx, inbound_rx) = mpsc::channel(256);
        let (outbound_tx, outbound_rx) = mpsc::channel(256);
        Self {
            inbound_tx,
            inbound_rx: std::sync::Arc::new(tokio::sync::Mutex::new(inbound_rx)),
            outbound_tx,
            outbound_rx: std::sync::Arc::new(tokio::sync::Mutex::new(outbound_rx)),
        }
    }

    pub async fn publish_inbound(&self, msg: InboundMessage) {
        let _ = self.inbound_tx.send(msg).await;
    }

    pub async fn consume_inbound(&self) -> Option<InboundMessage> {
        self.inbound_rx.lock().await.recv().await
    }

    pub async fn publish_outbound(&self, msg: OutboundMessage) {
        let _ = self.outbound_tx.send(msg).await;
    }

    pub async fn consume_outbound(&self) -> Option<OutboundMessage> {
        self.outbound_rx.lock().await.recv().await
    }

    pub fn outbound_sender(&self) -> mpsc::Sender<OutboundMessage> {
        self.outbound_tx.clone()
    }
}
