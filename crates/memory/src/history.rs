use std::path::{Path, PathBuf};

use uuid::Uuid;

use shared::errors::{PantherError, PantherResult};
use shared::types::{ChatHistory, Message};

#[derive(Clone)]
pub struct HistoryStore {
    path: PathBuf,
}

impl HistoryStore {
    pub fn new(base: &Path) -> Self {
        Self { path: base.to_path_buf() }
    }

    fn session_path(&self, session_id: Uuid) -> PathBuf {
        self.path.join(format!("{}.json", session_id))
    }

    pub async fn save_session(&self, history: &ChatHistory) -> PantherResult<()> {
        let target = self.session_path(history.session_id);
        let tmp = target.with_extension("tmp");
        let json = serde_json::to_string_pretty(history)?;
        tokio::fs::write(&tmp, json).await?;
        tokio::fs::rename(&tmp, &target).await?;
        Ok(())
    }

    pub async fn load_session(&self, session_id: Uuid) -> PantherResult<ChatHistory> {
        let target = self.session_path(session_id);
        if !target.exists() {
            return Err(PantherError::MemoryError(format!("Session {} not found", session_id)));
        }
        let raw = tokio::fs::read_to_string(&target).await?;
        let history = serde_json::from_str(&raw)?;
        Ok(history)
    }

    pub async fn load_recent(&self, limit: usize) -> PantherResult<Vec<ChatHistory>> {
        let mut sessions: Vec<ChatHistory> = Vec::new();

        let mut entries = tokio::fs::read_dir(&self.path).await?;
        while let Some(entry) = entries.next_entry().await? {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("json") {
                let raw = tokio::fs::read_to_string(&p).await?;
                if let Ok(h) = serde_json::from_str::<ChatHistory>(&raw) {
                    sessions.push(h);
                }
            }
        }

        sessions.sort_by(|a, b| {
            let ts_a = a.messages.last().map(|m| m.timestamp);
            let ts_b = b.messages.last().map(|m| m.timestamp);
            ts_b.cmp(&ts_a)
        });

        sessions.truncate(limit);
        Ok(sessions)
    }

    pub async fn append_message(&self, session_id: Uuid, message: Message) -> PantherResult<()> {
        let mut history = match self.load_session(session_id).await {
            Ok(h) => h,
            Err(_) => ChatHistory { session_id, messages: Vec::new() },
        };
        history.messages.push(message);
        self.save_session(&history).await
    }
}
