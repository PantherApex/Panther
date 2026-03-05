use thiserror::Error;

#[derive(Debug, Error)]
pub enum PantherError {
    #[error("LLM error: {0}")]
    LLMError(String),

    #[error("Memory error: {0}")]
    MemoryError(String),

    #[error("Skill error: {0}")]
    SkillError(String),

    #[error("Telegram error: {0}")]
    TelegramError(String),

    #[error("Execution error: {0}")]
    ExecutionError(String),

    #[error("Context engine error: {0}")]
    ContextError(String),

    #[error("Config error: {0}")]
    ConfigError(String),

    #[error("Channel error: {0}")]
    ChannelError(String),

    #[error("Event bus error: {0}")]
    EventBusError(String),

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

pub type PantherResult<T> = Result<T, PantherError>;
