use std::path::PathBuf;
use std::collections::HashMap;
use std::sync::Arc;
use chrono::{DateTime, Utc};
use tokio::sync::Mutex;
use serde::{Deserialize, Serialize};
use shared::types::LLMMessage;
use shared::errors::{PantherError, PantherResult};

#[derive(Serialize, Deserialize, Clone)]
pub struct Session {
    pub key: String,
    pub messages: Vec<LLMMessage>,
    pub last_consolidated: usize,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Session {
    pub fn new(key: String) -> Self {
        let now = Utc::now();
        Self {
            key,
            messages: Vec::new(),
            last_consolidated: 0,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn get_history(&self, max_messages: usize) -> Vec<LLMMessage> {
        let unconsolidated = &self.messages[self.last_consolidated..];
        let sliced = if unconsolidated.len() > max_messages {
            &unconsolidated[unconsolidated.len() - max_messages..]
        } else {
            unconsolidated
        };

        let start = sliced.iter().position(|m| m.role == "user").unwrap_or(0);
        sliced[start..].to_vec()
    }

    pub fn unconsolidated_count(&self) -> usize {
        self.messages.len().saturating_sub(self.last_consolidated)
    }

    pub fn clear(&mut self) {
        self.messages.clear();
        self.last_consolidated = 0;
        self.updated_at = Utc::now();
    }
}

#[derive(Serialize, Deserialize)]
struct SessionFile {
    key: String,
    last_consolidated: usize,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    messages: Vec<LLMMessage>,
}

#[derive(Clone)]
pub struct SessionStore {
    sessions_dir: PathBuf,
    cache: Arc<Mutex<HashMap<String, Session>>>,
    tool_result_truncation: usize,
}

impl SessionStore {
    pub fn new(sessions_dir: PathBuf, tool_result_truncation: usize) -> Self {
        Self {
            sessions_dir,
            cache: Arc::new(Mutex::new(HashMap::new())),
            tool_result_truncation,
        }
    }

    pub async fn get_or_create(&self, key: &str) -> Session {
        {
            let cache = self.cache.lock().await;
            if let Some(session) = cache.get(key) {
                return session.clone();
            }
        }
        let path = self.sessions_dir.join(format!("{}.json", sanitize_key(key)));
        if let Ok(raw) = tokio::fs::read_to_string(&path).await {
            if let Ok(file) = serde_json::from_str::<SessionFile>(&raw) {
                let session = Session {
                    key: file.key,
                    messages: file.messages,
                    last_consolidated: file.last_consolidated,
                    created_at: file.created_at,
                    updated_at: file.updated_at,
                };
                let mut cache = self.cache.lock().await;
                cache.insert(key.to_string(), session.clone());
                return session;
            }
        }
        Session::new(key.to_string())
    }

    pub async fn save(&self, session: &Session) -> PantherResult<()> {
        let limit = self.tool_result_truncation;
        let messages: Vec<LLMMessage> = session.messages.iter().cloned().map(|mut msg| {
            if msg.role == "tool" {
                if let Some(content) = &msg.content {
                    if content.chars().count() > limit {
                        let safe: String = content.chars().take(limit).collect();
                        msg.content = Some(format!("{}... (truncated)", safe));
                    }
                }
            }
            msg
        }).collect();

        let file = SessionFile {
            key: session.key.clone(),
            last_consolidated: session.last_consolidated,
            created_at: session.created_at,
            updated_at: session.updated_at,
            messages,
        };

        let json = serde_json::to_string(&file)
            .map_err(|e| PantherError::ConfigError(e.to_string()))?;

        tokio::fs::create_dir_all(&self.sessions_dir).await
            .map_err(|e| PantherError::IoError(e))?;

        let path = self.sessions_dir.join(format!("{}.json", sanitize_key(&session.key)));
        let tmp = path.with_extension("json.tmp");
        tokio::fs::write(&tmp, &json).await
            .map_err(|e| PantherError::IoError(e))?;
        tokio::fs::rename(&tmp, &path).await
            .map_err(|e| PantherError::IoError(e))?;

        let mut cache = self.cache.lock().await;
        cache.insert(session.key.clone(), session.clone());
        Ok(())
    }

    pub async fn save_and_invalidate(&self, session: &Session) -> PantherResult<()> {
        self.save(session).await?;
        let mut cache = self.cache.lock().await;
        cache.remove(&session.key);
        Ok(())
    }

    pub async fn clear(&self, key: &str) -> PantherResult<()> {
        {
            let mut cache = self.cache.lock().await;
            cache.remove(key);
        }
        let path = self.sessions_dir.join(format!("{}.json", sanitize_key(key)));
        if path.exists() {
            tokio::fs::remove_file(&path).await
                .map_err(|e| PantherError::IoError(e))?;
        }
        Ok(())
    }
}

fn sanitize_key(key: &str) -> String {
    key.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}
