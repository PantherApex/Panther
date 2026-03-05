use std::collections::HashSet;
use std::sync::Arc;

use agent::Agent;
use memory::MemoryStore;
use providers::TranscriptionProvider;
use shared::bus::MessageBus;
use shared::errors::PantherResult;
use teloxide::prelude::*;

pub const TELEGRAM_CHANNEL: &str = "telegram";

#[derive(Clone)]
pub struct PantherBot {
    pub agent: Arc<Agent>,
    pub token: String,
    pub bus: MessageBus,
    pub known_chats: Arc<tokio::sync::RwLock<std::collections::HashMap<String, i64>>>,
    pub memory: MemoryStore,
    pub allow_from: Arc<HashSet<String>>,
    pub transcription: Option<Arc<dyn TranscriptionProvider>>,
}

impl PantherBot {
    pub async fn new(
        agent: Arc<Agent>,
        token: String,
        bus: MessageBus,
        memory: MemoryStore,
        allow_from: Vec<String>,
        transcription: Option<Arc<dyn TranscriptionProvider>>,
    ) -> Self {
        let initial_chats = memory.load_known_chats().await;
        Self {
            agent,
            token,
            bus,
            known_chats: Arc::new(tokio::sync::RwLock::new(initial_chats)),
            memory,
            allow_from: Arc::new(allow_from.into_iter().collect()),
            transcription,
        }
    }

    pub fn is_allowed(&self, sender_id: &str) -> bool {
        if self.allow_from.is_empty() {
            return true;
        }
        self.allow_from.contains(sender_id)
    }

    pub async fn run(self) -> PantherResult<()> {
        if let Some(home) = dirs::home_dir() {
            tokio::fs::create_dir_all(home.join(".panther").join("temp")).await?;
        }

        let bot = teloxide::Bot::new(&self.token);
        let handler = Update::filter_message().endpoint(crate::handlers::handle_message);

        Dispatcher::builder(bot, handler)
            .dependencies(dptree::deps![self])
            .enable_ctrlc_handler()
            .build()
            .dispatch()
            .await;

        Ok(())
    }
}
