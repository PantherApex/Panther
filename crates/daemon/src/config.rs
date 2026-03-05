use serde::{Deserialize, Serialize};
use shared::errors::{PantherError, PantherResult};
use shared::types::LLMProvider;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SlackConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub app_token: String,
    #[serde(default)]
    pub bot_token: String,
    #[serde(default)]
    pub allow_from: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EmailConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub imap_host: String,
    #[serde(default = "default_imap_port")]
    pub imap_port: u16,
    #[serde(default)]
    pub imap_username: String,
    #[serde(default)]
    pub imap_password: String,
    #[serde(default = "default_inbox")]
    pub imap_mailbox: String,
    #[serde(default)]
    pub smtp_host: String,
    #[serde(default = "default_smtp_port")]
    pub smtp_port: u16,
    #[serde(default)]
    pub smtp_username: String,
    #[serde(default)]
    pub smtp_password: String,
    #[serde(default)]
    pub from_address: String,
    #[serde(default)]
    pub allow_from: Vec<String>,
    #[serde(default = "default_poll_interval_secs")]
    pub poll_interval_secs: u64,
    #[serde(default = "default_max_body_chars")]
    pub max_body_chars: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MatrixConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_matrix_homeserver")]
    pub homeserver: String,
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub allow_from: Vec<String>,
    #[serde(default = "default_matrix_group_policy")]
    pub group_policy: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CliConfig {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PantherConfig {
    pub telegram_token: String,
    pub active_provider: String,
    pub ollama_host: String,
    pub openai_key: String,
    pub anthropic_key: String,
    pub openrouter_key: String,
    pub gemini_key: String,
    pub ollama_model: String,
    pub cloud_model: String,
    pub groq_key: String,
    pub mistral_key: String,
    pub deepseek_key: String,
    pub xai_key: String,
    pub together_key: String,
    pub perplexity_key: String,
    pub cohere_key: String,
    #[serde(default)]
    pub brave_api_key: Option<String>,
    #[serde(default)]
    pub discord_token: Option<String>,
    #[serde(default)]
    pub telegram_allow_from: Vec<String>,
    #[serde(default)]
    pub discord_allow_from: Vec<String>,
    #[serde(default)]
    pub send_progress: bool,
    #[serde(default)]
    pub send_tool_hints: bool,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    #[serde(default = "default_memory_window")]
    pub memory_window: usize,
    #[serde(default = "default_tool_result_truncation")]
    pub tool_result_truncation: usize,
    #[serde(default = "default_exec_timeout_secs")]
    pub exec_timeout_secs: u64,
    #[serde(default)]
    pub exec_path_append: String,
    #[serde(default = "default_heartbeat_interval_secs")]
    pub heartbeat_interval_secs: u64,
    #[serde(default)]
    pub slack: SlackConfig,
    #[serde(default)]
    pub email: EmailConfig,
    #[serde(default)]
    pub matrix: MatrixConfig,
    #[serde(default)]
    pub cli: CliConfig,
    #[serde(default)]
    pub groq_transcription_key: String,
    #[serde(default = "default_transcription_model")]
    pub transcription_model: String,
}

impl PantherConfig {
    pub fn default_config() -> Self {
        Self {
            telegram_token: String::new(),
            active_provider: "ollama".to_string(),
            ollama_host: "http://localhost:11434".to_string(),
            openai_key: String::new(),
            anthropic_key: String::new(),
            openrouter_key: String::new(),
            gemini_key: String::new(),
            ollama_model: "llama3".to_string(),
            cloud_model: "gpt-4o".to_string(),
            groq_key: String::new(),
            mistral_key: String::new(),
            deepseek_key: String::new(),
            xai_key: String::new(),
            together_key: String::new(),
            perplexity_key: String::new(),
            cohere_key: String::new(),
            brave_api_key: None,
            discord_token: None,
            telegram_allow_from: Vec::new(),
            discord_allow_from: Vec::new(),
            send_progress: false,
            send_tool_hints: false,
            mcp_servers: Vec::new(),
            max_iterations: default_max_iterations(),
            max_tokens: default_max_tokens(),
            temperature: default_temperature(),
            memory_window: default_memory_window(),
            tool_result_truncation: default_tool_result_truncation(),
            exec_timeout_secs: default_exec_timeout_secs(),
            exec_path_append: String::new(),
            heartbeat_interval_secs: default_heartbeat_interval_secs(),
            slack: SlackConfig::default(),
            email: EmailConfig::default(),
            matrix: MatrixConfig::default(),
            cli: CliConfig::default(),
            groq_transcription_key: String::new(),
            transcription_model: default_transcription_model(),
        }
    }

    pub async fn load() -> PantherResult<Self> {
        let path = config_path()?;
        if !path.exists() {
            let defaults = Self::default_config();
            defaults.save().await?;
            return Ok(defaults);
        }
        let content = tokio::fs::read_to_string(&path).await?;
        toml::from_str(&content).map_err(|e| PantherError::ConfigError(e.to_string()))
    }

    pub async fn save(&self) -> PantherResult<std::path::PathBuf> {
        let path = config_path()?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let content =
            toml::to_string(self).map_err(|e| PantherError::ConfigError(e.to_string()))?;
        let tmp = path.with_extension("toml.tmp");
        tokio::fs::write(&tmp, content.as_bytes()).await?;
        tokio::fs::rename(&tmp, &path).await?;
        Ok(path)
    }

    #[allow(dead_code)]
    pub fn to_llm_provider(&self) -> LLMProvider {
        match self.active_provider.to_lowercase().as_str() {
            "openai" => LLMProvider::OpenAI,
            "anthropic" => LLMProvider::Anthropic,
            "openrouter" => LLMProvider::OpenRouter,
            "gemini" => LLMProvider::Gemini,
            "groq" => LLMProvider::Groq,
            "mistral" => LLMProvider::Mistral,
            "deepseek" => LLMProvider::DeepSeek,
            "xai" => LLMProvider::XAI,
            "together" => LLMProvider::TogetherAI,
            "perplexity" => LLMProvider::Perplexity,
            "cohere" => LLMProvider::Cohere,
            _ => LLMProvider::Ollama,
        }
    }

    #[allow(dead_code)]
    pub fn active_model(&self) -> String {
        if self.active_provider.to_lowercase() == "ollama" {
            self.ollama_model.clone()
        } else {
            self.cloud_model.clone()
        }
    }
}

fn config_path() -> PantherResult<std::path::PathBuf> {
    dirs::home_dir()
        .map(|h| h.join(".panther").join("config.toml"))
        .ok_or_else(|| PantherError::ConfigError("Cannot determine home directory".into()))
}

fn default_max_iterations() -> usize { 40 }
fn default_max_tokens() -> u32 { 8096 }
fn default_temperature() -> f32 { 0.1 }
fn default_memory_window() -> usize { 100 }
fn default_tool_result_truncation() -> usize { 500 }
fn default_exec_timeout_secs() -> u64 { 30 }
fn default_heartbeat_interval_secs() -> u64 { 1800 }
fn default_imap_port() -> u16 { 993 }
fn default_smtp_port() -> u16 { 587 }
fn default_inbox() -> String { "INBOX".to_string() }
fn default_poll_interval_secs() -> u64 { 30 }
fn default_max_body_chars() -> usize { 12000 }
fn default_matrix_homeserver() -> String { "https://matrix.org".to_string() }
fn default_matrix_group_policy() -> String { "mention".to_string() }
fn default_transcription_model() -> String { "whisper-large-v3".to_string() }

