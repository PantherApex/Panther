use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use agent::{Agent, ContextBuilder, CronService, HeartbeatService, SessionStore, SubagentManager};
use agent::cron::service::JobHandler;
use agent::heartbeat::{ExecuteFn, NotifyFn};
use agent::subagent::AnnounceFn;
use agent::tools::cron::CronTool;
use agent::tools::exec::ExecTool;
use agent::tools::filesystem::{ReadFileTool, WriteFileTool, EditFileTool, ListDirTool};
use agent::tools::message::MessageTool;
use agent::tools::registry::ToolRegistry;
use agent::tools::skill::ReadSkillTool;
use agent::tools::spawn::SpawnTool;
use agent::tools::web::{WebSearchTool, WebFetchTool};
use agent::tools::capture::CaptureMediaTool;
use agent::tools::send_file::SendFileTool;
use agent::tools::clipboard::ClipboardTool;
use agent::tools::system_info::SystemInfoTool;
use bot::{PantherBot, OutboundDispatcher};
use bot::channels::{
    TelegramChannel, DiscordChannel,
    SlackChannel, EmailChannel, EmailConfig as BotEmailConfig,
    MatrixChannel, MatrixConfig as BotMatrixConfig, MatrixGroupPolicy,
    CliChannel,
};
use bot::discord_bot::DiscordBot;
use context_engine::ContextEngine;
use memory::MemoryStore;
use providers::{ProviderRouter, GroqTranscriptionProvider, TranscriptionProvider};
use shared::bus::MessageBus;
use shared::channel::Channel;
use shared::errors::PantherResult;
use shared::events::EventBus;
use skill_runner::SkillRunner;

use crate::config::PantherConfig;

#[allow(dead_code)]
pub struct PantherSystem {
    pub memory: MemoryStore,
    pub providers: ProviderRouter,
    pub skill_runner: SkillRunner,
    pub agent: Arc<Agent>,
    pub bot: PantherBot,
    pub discord_bot: Option<DiscordBot>,
    pub outbound_dispatcher: OutboundDispatcher,
    pub context_engine: ContextEngine,
    pub event_bus: EventBus,
    pub cron: CronService,
    pub heartbeat: Arc<HeartbeatService>,
    pub config: PantherConfig,
    pub background_tasks: Vec<tokio::task::JoinHandle<()>>,
}

pub async fn init(config: PantherConfig) -> PantherResult<PantherSystem> {
    let event_bus = EventBus::new();
    let memory = MemoryStore::init().await?;
    let bus = MessageBus::new();

    let providers = ProviderRouter::new(
        config.to_llm_provider(),
        config.ollama_host.clone(),
        config.openai_key.clone(),
        config.anthropic_key.clone(),
        config.openrouter_key.clone(),
        config.gemini_key.clone(),
        config.groq_key.clone(),
        config.mistral_key.clone(),
        config.deepseek_key.clone(),
        config.xai_key.clone(),
        config.together_key.clone(),
        config.perplexity_key.clone(),
        config.cohere_key.clone(),
        config.ollama_model.clone(),
        config.cloud_model.clone(),
    );

    let skill_runner = SkillRunner::new(memory.clone());

    let workspace = dirs::home_dir()
        .expect("Cannot determine home directory")
        .join(".panther")
        .join("workspace");
    tokio::fs::create_dir_all(&workspace).await?;
    tokio::fs::create_dir_all(workspace.join("memory")).await?;
    tokio::fs::create_dir_all(workspace.join("skills")).await?;
    ensure_identity_files(&workspace).await;

    let sessions_dir = dirs::home_dir()
        .expect("Cannot determine home directory")
        .join(".panther")
        .join("sessions");
    tokio::fs::create_dir_all(&sessions_dir).await?;

    let working_dir = workspace.to_string_lossy().to_string();

    let subagent_manager = Arc::new(SubagentManager::new(
        providers.clone(),
        workspace.clone(),
        config.brave_api_key.clone(),
        config.exec_timeout_secs,
        config.exec_path_append.clone(),
        config.temperature,
        config.max_tokens,
        config.max_iterations,
        bus.clone(),
    ));

    let cron_path = workspace.join("cron_jobs.json");
    let cron_agent_slot: Arc<tokio::sync::Mutex<Option<Arc<Agent>>>> = Arc::new(tokio::sync::Mutex::new(None));

    let cron_agent_ref = Arc::clone(&cron_agent_slot);
    let cron_handler: JobHandler = Arc::new(move |job| {
        let slot = Arc::clone(&cron_agent_ref);
        Box::pin(async move {
            let guard = slot.lock().await;
            if let Some(agent) = guard.as_ref() {
                let session_key = format!("{}:{}", job.payload.channel, job.payload.chat_id);
                let _ = agent.dispatch_direct(
                    session_key,
                    job.payload.channel,
                    job.payload.chat_id,
                    job.payload.message,
                    None,
                    None,
                ).await;
            }
        })
    });

    let cron = CronService::new(cron_path).with_handler(cron_handler);

    let mut channel_map: HashMap<String, Arc<dyn Channel>> = HashMap::new();
    let mut background_tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    if !config.telegram_token.is_empty() {
        channel_map.insert("telegram".to_string(), Arc::new(TelegramChannel::new(config.telegram_token.clone())));
    }

    if let Some(ref discord_token) = config.discord_token {
        channel_map.insert("discord".to_string(), Arc::new(DiscordChannel::new(discord_token.clone())));
    }

    if config.slack.enabled && !config.slack.app_token.is_empty() && !config.slack.bot_token.is_empty() {
        let slack = Arc::new(SlackChannel::new(
            config.slack.app_token.clone(),
            config.slack.bot_token.clone(),
            config.slack.allow_from.clone(),
            bus.clone(),
        ));
        channel_map.insert("slack".to_string(), Arc::clone(&slack) as Arc<dyn Channel>);
        let slack_loop = Arc::clone(&slack);
        background_tasks.push(tokio::spawn(async move { slack_loop.run_loop().await }));
        eprintln!("[panther:slack] channel initialized");
    }

    if config.email.enabled && !config.email.imap_host.is_empty() {
        let email_cfg = BotEmailConfig {
            imap_host: config.email.imap_host.clone(),
            imap_port: config.email.imap_port,
            imap_username: config.email.imap_username.clone(),
            imap_password: config.email.imap_password.clone(),
            imap_mailbox: config.email.imap_mailbox.clone(),
            smtp_host: config.email.smtp_host.clone(),
            smtp_port: config.email.smtp_port,
            smtp_username: config.email.smtp_username.clone(),
            smtp_password: config.email.smtp_password.clone(),
            from_address: config.email.from_address.clone(),
            allow_from: config.email.allow_from.clone(),
            poll_interval_secs: config.email.poll_interval_secs,
            max_body_chars: config.email.max_body_chars,
        };
        let email = Arc::new(EmailChannel::new(email_cfg, bus.clone()));
        channel_map.insert("email".to_string(), Arc::clone(&email) as Arc<dyn Channel>);
        let email_loop = Arc::clone(&email);
        background_tasks.push(tokio::spawn(async move { email_loop.run_loop().await }));
        eprintln!("[panther:email] channel initialized");
    }

    if config.matrix.enabled && !config.matrix.access_token.is_empty() {
        let group_policy = match config.matrix.group_policy.as_str() {
            "open" => MatrixGroupPolicy::Open,
            "allowlist" => MatrixGroupPolicy::AllowList,
            _ => MatrixGroupPolicy::MentionOnly,
        };
        let matrix_cfg = BotMatrixConfig {
            homeserver: config.matrix.homeserver.clone(),
            access_token: config.matrix.access_token.clone(),
            user_id: config.matrix.user_id.clone(),
            allow_from: config.matrix.allow_from.clone(),
            group_policy,
        };
        let matrix = Arc::new(MatrixChannel::new(matrix_cfg, bus.clone()));
        channel_map.insert("matrix".to_string(), Arc::clone(&matrix) as Arc<dyn Channel>);
        let matrix_loop = Arc::clone(&matrix);
        background_tasks.push(tokio::spawn(async move { matrix_loop.run_loop().await }));
        eprintln!("[panther:matrix] channel initialized");
    }

    if config.cli.enabled {
        let cli = Arc::new(CliChannel::new(bus.clone()));
        channel_map.insert("cli".to_string(), Arc::clone(&cli) as Arc<dyn Channel>);
        let cli_loop = Arc::clone(&cli);
        background_tasks.push(tokio::spawn(async move { cli_loop.run_loop().await }));
        eprintln!("[panther:cli] channel initialized");
    }

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ExecTool::new(working_dir.clone(), config.exec_timeout_secs, config.exec_path_append.clone())));
    registry.register(Box::new(ReadFileTool));
    registry.register(Box::new(WriteFileTool));
    registry.register(Box::new(EditFileTool));
    registry.register(Box::new(ListDirTool));
    registry.register(Box::new(WebSearchTool::new(config.brave_api_key.clone())));
    registry.register(Box::new(WebFetchTool::new()));
    registry.register(Box::new(ReadSkillTool::new(workspace.join("skills"))));
    registry.register(Box::new(ClipboardTool));
    registry.register(Box::new(SystemInfoTool));

    let temp_dir = dirs::home_dir()
        .expect("Cannot determine home directory")
        .join(".panther")
        .join("temp");
    registry.register_capture_tool(std::sync::Arc::new(CaptureMediaTool::new(temp_dir)));

    registry.register_message_tool(Arc::new(MessageTool::new(bus.clone())));
    registry.register_cron_tool(Arc::new(CronTool::new(cron.clone())));
    registry.register_spawn_tool(Arc::new(SpawnTool::new(Arc::clone(&subagent_manager))));
    registry.register_send_file_tool(Arc::new(SendFileTool::new(bus.clone())));

    for server_cfg in &config.mcp_servers {
        match mcp::McpClient::spawn(
            server_cfg.name.clone(),
            &server_cfg.command,
            &server_cfg.args,
        ).await {
            Ok(client) => {
                let client = Arc::new(client);
                match client.list_tools().await {
                    Ok(tools) => {
                        let count = tools.len();
                        for tool_info in tools {
                            let params = tool_info.input_schema.clone().unwrap_or_else(|| {
                                serde_json::json!({ "type": "object", "properties": {} })
                            });
                            registry.register(Box::new(agent::tools::mcp::McpTool::new(
                                Arc::clone(&client),
                                tool_info.name,
                                tool_info.description.unwrap_or_default(),
                                params,
                            )));
                        }
                        eprintln!("[panther:mcp] Server '{}' connected: {} tool(s) registered.", server_cfg.name, count);
                    }
                    Err(e) => {
                        eprintln!("[panther:mcp] Server '{}' connected but tools/list failed: {}", server_cfg.name, e);
                    }
                }
            }
            Err(e) => {
                eprintln!("[panther:mcp] Failed to start server '{}': {}", server_cfg.name, e);
            }
        }
    }

    let session_store = SessionStore::new(sessions_dir, config.tool_result_truncation);
    let context_builder = ContextBuilder::new(workspace.clone());

    let agent = Arc::new(
        Agent::new(
            providers.clone(),
            registry,
            session_store,
            context_builder,
            workspace.clone(),
            bus.clone(),
            event_bus.clone(),
            config.max_iterations,
            config.max_tokens,
            config.temperature,
            config.memory_window,
            config.send_progress,
        )
        .with_subagents(Arc::clone(&subagent_manager))
    );

    *cron_agent_slot.lock().await = Some(Arc::clone(&agent));

    let announce_agent = Arc::clone(&agent);
    let announce_fn: AnnounceFn = Arc::new(move |channel, chat_id, content| {
        let agent = Arc::clone(&announce_agent);
        Box::pin(async move {
            let session_key = format!("{}:{}", channel, chat_id);
            let _ = agent.dispatch_direct(session_key, channel, chat_id, content, None, None).await;
        })
    });
    subagent_manager.set_announce(announce_fn);

    let heartbeat_agent = Arc::clone(&agent);
    let execute_fn: ExecuteFn = Arc::new(move |tasks| {
        let agent = Arc::clone(&heartbeat_agent);
        Box::pin(async move {
            agent.dispatch_direct(
                "heartbeat:system".to_string(),
                "heartbeat".to_string(),
                "system".to_string(),
                tasks,
                None,
                None,
            )
            .await
            .unwrap_or_else(|e| format!("Heartbeat error: {}", e))
        })
    });

    let heartbeat_notify_bus = bus.clone();
    let notify_fn: NotifyFn = Arc::new(move |response| {
        let bus = heartbeat_notify_bus.clone();
        Box::pin(async move {
            bus.publish_outbound(shared::bus::OutboundMessage::text("heartbeat", "system", response)).await;
        })
    });

    let heartbeat = Arc::new(
        HeartbeatService::new(workspace.clone(), providers.clone(), config.heartbeat_interval_secs)
            .with_execute(execute_fn)
            .with_notify(notify_fn)
    );

    let context_engine = ContextEngine::new(memory.clone(), providers.clone(), event_bus.clone());

    let outbound_dispatcher = OutboundDispatcher::new(
        bus.clone(),
        channel_map,
        config.send_progress,
        config.send_tool_hints,
        config.telegram_token.clone(),
    );

    let bot = PantherBot::new(
        Arc::clone(&agent),
        config.telegram_token.clone(),
        bus.clone(),
        memory.clone(),
        config.telegram_allow_from.clone(),
        build_transcription_provider(&config),
    ).await;

    let discord_bot = config.discord_token.as_ref().map(|dt| {
        DiscordBot::new(dt.clone(), bus.clone(), config.discord_allow_from.clone())
    });

    Ok(PantherSystem {
        memory,
        providers,
        skill_runner,
        agent,
        bot,
        discord_bot,
        outbound_dispatcher,
        context_engine,
        event_bus,
        cron,
        heartbeat,
        config,
        background_tasks,
    })
}

fn build_transcription_provider(config: &PantherConfig) -> Option<Arc<dyn TranscriptionProvider>> {
    let key = if !config.groq_transcription_key.is_empty() {
        config.groq_transcription_key.clone()
    } else if !config.groq_key.is_empty() {
        config.groq_key.clone()
    } else {
        return None;
    };
    Some(Arc::new(GroqTranscriptionProvider::new(key, config.transcription_model.clone())))
}

async fn ensure_identity_files(workspace: &PathBuf) {
    let files: &[(&str, &str)] = &[
        ("SOUL.md", ""),
        ("USER.md", "# User\n\nAdd facts about yourself here. Panther will always remember these.\n"),
        ("AGENTS.md", "# Agents\n\nSubagents can be spawned for background tasks using the spawn tool.\n"),
        ("TOOLS.md", "# Tools\n\n## Available Native Tools\n\n- **exec** — Run shell commands\n- **read_file** — Read a file\n- **write_file** — Write/create a file\n- **edit_file** — Edit a file (exact string replacement)\n- **list_dir** — List directory contents\n- **web_search** — Search the web via Brave API\n- **web_fetch** — Fetch a URL\n- **message** — Send a message mid-task\n- **read_skill** — Load full instructions for a named skill\n- **cron** — Schedule reminders and recurring tasks (add/list/remove)\n- **spawn** — Spawn a background subagent for complex long-running tasks\n"),
        ("HEARTBEAT.md", "# Heartbeat\n\nList active background tasks here. Panther checks this file periodically.\n"),
    ];

    for (name, content) in files {
        let path = workspace.join(name);
        if !path.exists() && !content.is_empty() {
            let _ = tokio::fs::write(&path, content).await;
        }
    }
}

