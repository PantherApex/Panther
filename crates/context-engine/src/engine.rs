use memory::MemoryStore;
use providers::ProviderRouter;
use shared::events::EventBus;

#[derive(Clone)]
pub struct ContextEngine {
    _memory: MemoryStore,
    _providers: ProviderRouter,
    _event_bus: EventBus,
}

impl ContextEngine {
    pub fn new(memory: MemoryStore, providers: ProviderRouter, event_bus: EventBus) -> Self {
        Self {
            _memory: memory,
            _providers: providers,
            _event_bus: event_bus,
        }
    }

    pub async fn start(self) -> shared::errors::PantherResult<()> {
        Ok(())
    }
}
