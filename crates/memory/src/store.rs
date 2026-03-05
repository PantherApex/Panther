use std::collections::HashMap;
use std::path::PathBuf;

use shared::errors::{PantherError, PantherResult};

use crate::{HistoryStore, ProfileStore, SkillStore};

#[derive(Clone)]
pub struct MemoryStore {
    pub profile: ProfileStore,
    pub history: HistoryStore,
    pub skills: SkillStore,
    pub chats_path: PathBuf,
}

impl MemoryStore {
    pub async fn init() -> PantherResult<Self> {
        let home = dirs::home_dir()
            .ok_or_else(|| PantherError::ConfigError("Cannot determine home directory".into()))?;

        let base: PathBuf = home.join(".panther");

        for sub in ["profile", "history", "skills", "chats"] {
            tokio::fs::create_dir_all(base.join(sub)).await?;
        }

        Ok(Self {
            profile: ProfileStore::new(&base.join("profile")),
            history: HistoryStore::new(&base.join("history")),
            skills: SkillStore::new(&base.join("skills")),
            chats_path: base.join("chats").join("known_chats.json"),
        })
    }

    pub async fn load_known_chats(&self) -> HashMap<String, i64> {
        let Ok(raw) = tokio::fs::read_to_string(&self.chats_path).await else {
            return HashMap::new();
        };
        serde_json::from_str(&raw).unwrap_or_default()
    }

    pub async fn save_known_chats(&self, map: &HashMap<String, i64>) {
        if let Ok(json) = serde_json::to_string(map) {
            let _ = tokio::fs::write(&self.chats_path, json).await;
        }
    }
}
