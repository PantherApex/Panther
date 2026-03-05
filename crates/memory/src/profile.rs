use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::Utc;
use shared::errors::PantherResult;
use shared::types::UserProfile;

#[derive(Clone)]
pub struct ProfileStore {
    path: PathBuf,
}

impl ProfileStore {
    pub fn new(base: &Path) -> Self {
        Self { path: base.join("profile.json") }
    }

    pub async fn load(&self) -> PantherResult<UserProfile> {
        if !self.path.exists() {
            return Ok(UserProfile {
                name: None,
                preferences: HashMap::new(),
                personality_notes: Vec::new(),
                known_projects: Vec::new(),
                communication_style: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            });
        }
        let raw = tokio::fs::read_to_string(&self.path).await?;
        let profile = serde_json::from_str(&raw)?;
        Ok(profile)
    }

    pub async fn save(&self, profile: &UserProfile) -> PantherResult<()> {
        let tmp = self.path.with_extension("tmp");
        let json = serde_json::to_string_pretty(profile)?;
        tokio::fs::write(&tmp, json).await?;
        tokio::fs::rename(&tmp, &self.path).await?;
        Ok(())
    }

    pub async fn update<F>(&self, f: F) -> PantherResult<UserProfile>
    where
        F: FnOnce(&mut UserProfile),
    {
        let mut profile = self.load().await?;
        f(&mut profile);
        profile.updated_at = Utc::now();
        self.save(&profile).await?;
        Ok(profile)
    }
}
