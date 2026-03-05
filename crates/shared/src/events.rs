use tokio::sync::broadcast;

use crate::errors::PantherResult;
use crate::types::{LLMProvider, Message};

#[derive(Debug, Clone)]
pub enum PantherEvent {
    MessageReceived(Message),
    MemoryUpdated,
    ProviderSwitched(LLMProvider),
    ShutdownRequested,
}

#[derive(Clone)]
pub struct EventBus {
    sender: broadcast::Sender<PantherEvent>,
}

impl EventBus {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(256);
        Self { sender }
    }

    pub fn publish(&self, event: PantherEvent) -> PantherResult<()> {
        match self.sender.send(event) {
            Ok(_) => Ok(()),
            Err(_) => Ok(()),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<PantherEvent> {
        self.sender.subscribe()
    }
}
