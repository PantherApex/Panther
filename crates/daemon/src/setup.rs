use std::path::PathBuf;
use dialoguer::theme::ColorfulTheme;
use dialoguer::{Confirm, Input, Password, Select};

use crate::config::{McpServerConfig, PantherConfig};

const PROVIDERS: &[(&str, &str, &str)] = &[
    ("ollama",      "Ollama (local, private)",               ""),
    ("openai",      "OpenAI (GPT-4o etc.)",                  "sk-..."),
    ("anthropic",   "Anthropic (Claude)",                    "sk-ant-..."),
    ("openrouter",  "OpenRouter (multi-model gateway)",      "sk-or-..."),
    ("gemini",      "Google Gemini",                         "AI..."),
    ("groq",        "Groq (fast inference)",                 "gsk_..."),
    ("mistral",     "Mistral",                               "..."),
    ("deepseek",    "DeepSeek",                              "sk-..."),
    ("xai",         "xAI / Grok",                            "xai-..."),
    ("together",    "Together AI",                           "..."),
    ("perplexity",  "Perplexity",                            "pplx-..."),
    ("cohere",      "Cohere",                                "..."),
];

const DEFAULT_MODELS: &[(&str, &str)] = &[
    ("ollama",      "llama3"),
    ("openai",      "gpt-4o"),
    ("anthropic",   "claude-sonnet-4-5"),
    ("openrouter",  "anthropic/claude-sonnet-4-5"),
    ("gemini",      "gemini-1.5-pro"),
    ("groq",        "llama-3.1-70b-versatile"),
    ("mistral",     "mistral-large-latest"),
    ("deepseek",    "deepseek-chat"),
    ("xai",         "grok-beta"),
    ("together",    "meta-llama/Llama-3-70b-chat-hf"),
    ("perplexity",  "llama-3.1-sonar-large-128k-online"),
    ("cohere",      "command-r-plus"),
];

pub fn run(mut cfg: PantherConfig) -> PantherConfig {
    let theme = ColorfulTheme::default();

    println!();
    println!("╔══════════════════════════════════════════╗");
    println!("║   🐆  Panther Setup                       ║");
    println!("╚══════════════════════════════════════════╝");
    println!();
    println!("Answer each question to configure your agent.");
    println!("Press Enter to accept [defaults]. Passwords are hidden while typing.");
    println!();

    println!("── Step 1: LLM Provider ────────────────────");
    let provider_idx = select_provider(&theme, &cfg.active_provider);
    let provider_name = PROVIDERS[provider_idx].0;
    cfg.active_provider = provider_name.to_string();

    if provider_name == "ollama" {
        cfg.ollama_host = Input::with_theme(&theme)
            .with_prompt("Ollama host URL")
            .default(if cfg.ollama_host.is_empty() { "http://localhost:11434".to_string() } else { cfg.ollama_host.clone() })
            .interact_text()
            .unwrap_or_else(|_| cfg.ollama_host.clone());

        cfg.ollama_model = Input::with_theme(&theme)
            .with_prompt("Ollama model")
            .default(default_model(provider_name, &cfg.ollama_model))
            .interact_text()
            .unwrap_or_else(|_| cfg.ollama_model.clone());

        if Confirm::with_theme(&theme)
            .with_prompt("Pull this model with 'ollama pull' now?")
            .default(false)
            .interact()
            .unwrap_or(false)
        {
            let _ = std::process::Command::new("ollama").args(["pull", &cfg.ollama_model]).status();
        }
    } else {
        let key = prompt_api_key(&theme, PROVIDERS[provider_idx].1);
        set_provider_key(provider_name, key, &mut cfg);
        cfg.cloud_model = Input::with_theme(&theme)
            .with_prompt("Model name")
            .default(default_model(provider_name, &cfg.cloud_model))
            .interact_text()
            .unwrap_or_else(|_| cfg.cloud_model.clone());
    }
    println!();

    println!("── Step 2: Channels ─────────────────────────");
    println!("  Configure which channels Panther listens on.");
    println!("  You need at least one to receive messages.");
    println!();

    cfg = setup_telegram(&theme, cfg);
    cfg = setup_discord(&theme, cfg);
    cfg = setup_slack(&theme, cfg);
    cfg = setup_email(&theme, cfg);
    cfg = setup_matrix(&theme, cfg);
    cfg = setup_cli(&theme, cfg);
    println!();

    println!("── Step 3: Optional Capabilities ──────────");

    let brave_key: String = Password::with_theme(&theme)
        .with_prompt("Brave Search API key for web_search (Enter to skip)")
        .allow_empty_password(true)
        .interact()
        .unwrap_or_default();
    if !brave_key.is_empty() {
        cfg.brave_api_key = Some(brave_key);
        println!("  → Web search enabled.");
    } else {
        println!("  → Web search skipped.");
    }

    let groq_transcription_hint = if !cfg.groq_transcription_key.is_empty() {
        "Groq transcription key (set) — Enter to keep, or paste new key to replace"
    } else if !cfg.groq_key.is_empty() {
        "Groq transcription key (using groq_key) — Enter to use existing groq_key, or paste dedicated key"
    } else {
        "Groq API key for voice transcription via Whisper (Enter to skip)"
    };
    let transcription_key: String = Password::with_theme(&theme)
        .with_prompt(groq_transcription_hint)
        .allow_empty_password(true)
        .interact()
        .unwrap_or_default();
    if !transcription_key.is_empty() {
        cfg.groq_transcription_key = transcription_key;
        println!("  → Voice transcription enabled (dedicated key).");
    } else if !cfg.groq_key.is_empty() {
        println!("  → Voice transcription enabled (using groq_key).");
    } else {
        println!("  → Voice transcription skipped.");
    }

    cfg.send_progress = Confirm::with_theme(&theme)
        .with_prompt("Send tool-use hints as progress messages?")
        .default(cfg.send_progress)
        .interact()
        .unwrap_or(false);

    println!();
    println!("── Step 4: MCP Servers (optional) ──────────");
    if Confirm::with_theme(&theme)
        .with_prompt("Add an MCP server?")
        .default(false)
        .interact()
        .unwrap_or(false)
    {
        cfg.mcp_servers = prompt_mcp_servers(&theme, cfg.mcp_servers);
    }

    cfg
}

fn setup_telegram(theme: &ColorfulTheme, mut cfg: PantherConfig) -> PantherConfig {
    let currently_set = !cfg.telegram_token.is_empty();
    let enable = Confirm::with_theme(theme)
        .with_prompt(if currently_set { "Telegram (configured) — reconfigure?" } else { "Enable Telegram?" })
        .default(currently_set)
        .interact()
        .unwrap_or(currently_set);

    if !enable {
        return cfg;
    }

    println!("  1. Search @BotFather in Telegram");
    println!("  2. /newbot — follow prompts");
    println!("  3. Copy the token (1234567890:ABCdef...)");
    println!();

    loop {
        let token: String = Password::with_theme(theme)
            .with_prompt("Telegram Bot Token (Enter to keep current)")
            .allow_empty_password(currently_set)
            .interact()
            .unwrap_or_default();
        if token.is_empty() && currently_set { break; }
        if token.contains(':') && token.len() > 10 {
            cfg.telegram_token = token;
            break;
        }
        println!("  ✗ Invalid format — tokens look like: 1234567890:ABCdef...");
    }

    let raw: String = Input::with_theme(theme)
        .with_prompt("Telegram allowed usernames, comma-separated (Enter = allow all)")
        .allow_empty(true)
        .default(cfg.telegram_allow_from.join(","))
        .interact_text()
        .unwrap_or_default();
    cfg.telegram_allow_from = raw.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();

    println!("  ✓ Telegram configured.");
    cfg
}

fn setup_discord(theme: &ColorfulTheme, mut cfg: PantherConfig) -> PantherConfig {
    let currently_set = cfg.discord_token.is_some();
    let enable = Confirm::with_theme(theme)
        .with_prompt(if currently_set { "Discord (configured) — reconfigure?" } else { "Enable Discord?" })
        .default(currently_set)
        .interact()
        .unwrap_or(false);

    if !enable { return cfg; }

    let token: String = Password::with_theme(theme)
        .with_prompt("Discord bot token (Enter to keep)")
        .allow_empty_password(currently_set)
        .interact()
        .unwrap_or_default();
    if !token.is_empty() { cfg.discord_token = Some(token); }

    let raw: String = Input::with_theme(theme)
        .with_prompt("Discord user IDs allowed, comma-separated (Enter = allow all)")
        .allow_empty(true)
        .default(cfg.discord_allow_from.join(","))
        .interact_text()
        .unwrap_or_default();
    cfg.discord_allow_from = raw.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();

    println!("  ✓ Discord configured.");
    cfg
}

fn setup_slack(theme: &ColorfulTheme, mut cfg: PantherConfig) -> PantherConfig {
    let currently_enabled = cfg.slack.enabled;
    let enable = Confirm::with_theme(theme)
        .with_prompt(if currently_enabled { "Slack (configured) — reconfigure?" } else { "Enable Slack?" })
        .default(currently_enabled)
        .interact()
        .unwrap_or(false);

    if !enable {
        if currently_enabled { cfg.slack.enabled = false; println!("  → Slack disabled."); }
        return cfg;
    }

    println!("  Setup: api.slack.com/apps → New App → Socket Mode");
    println!("  App-Level Token needs connections:write scope (starts with xapp-)");
    println!("  Bot Token needs chat:write, files:read, files:write (starts with xoxb-)");
    println!("  Subscribe to: message.channels, message.im, message.groups");
    println!();

    let app_token: String = Password::with_theme(theme)
        .with_prompt("Slack App-Level Token (xapp-...) (Enter to keep)")
        .allow_empty_password(currently_enabled)
        .interact()
        .unwrap_or_default();
    if !app_token.is_empty() { cfg.slack.app_token = app_token; }

    let bot_token: String = Password::with_theme(theme)
        .with_prompt("Slack Bot Token (xoxb-...) (Enter to keep)")
        .allow_empty_password(currently_enabled)
        .interact()
        .unwrap_or_default();
    if !bot_token.is_empty() { cfg.slack.bot_token = bot_token; }

    let raw: String = Input::with_theme(theme)
        .with_prompt("Slack user IDs allowed, comma-separated (Enter = allow all)")
        .allow_empty(true)
        .default(cfg.slack.allow_from.join(","))
        .interact_text()
        .unwrap_or_default();
    cfg.slack.allow_from = raw.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();

    cfg.slack.enabled = true;
    println!("  ✓ Slack configured.");
    cfg
}

fn setup_email(theme: &ColorfulTheme, mut cfg: PantherConfig) -> PantherConfig {
    let currently_enabled = cfg.email.enabled;
    let enable = Confirm::with_theme(theme)
        .with_prompt(if currently_enabled { "Email (configured) — reconfigure?" } else { "Enable Email (IMAP + SMTP)?" })
        .default(currently_enabled)
        .interact()
        .unwrap_or(false);

    if !enable {
        if currently_enabled { cfg.email.enabled = false; println!("  → Email disabled."); }
        return cfg;
    }

    println!("  Use a dedicated address or app-password. Panther polls IMAP and replies via SMTP.");
    println!();

    cfg.email.imap_host = Input::with_theme(theme)
        .with_prompt("IMAP host (e.g. imap.gmail.com)")
        .default(cfg.email.imap_host.clone())
        .interact_text()
        .unwrap_or_else(|_| cfg.email.imap_host.clone());

    cfg.email.imap_port = Input::with_theme(theme)
        .with_prompt("IMAP port")
        .default(cfg.email.imap_port)
        .interact_text()
        .unwrap_or(993);

    cfg.email.imap_username = Input::with_theme(theme)
        .with_prompt("IMAP username")
        .default(cfg.email.imap_username.clone())
        .interact_text()
        .unwrap_or_else(|_| cfg.email.imap_username.clone());

    let imap_pass: String = Password::with_theme(theme)
        .with_prompt("IMAP password (Enter to keep)")
        .allow_empty_password(currently_enabled)
        .interact()
        .unwrap_or_default();
    if !imap_pass.is_empty() { cfg.email.imap_password = imap_pass; }

    cfg.email.smtp_host = Input::with_theme(theme)
        .with_prompt("SMTP host (e.g. smtp.gmail.com)")
        .default(cfg.email.smtp_host.clone())
        .interact_text()
        .unwrap_or_else(|_| cfg.email.smtp_host.clone());

    cfg.email.smtp_port = Input::with_theme(theme)
        .with_prompt("SMTP port")
        .default(cfg.email.smtp_port)
        .interact_text()
        .unwrap_or(587);

    let default_smtp_user = if cfg.email.smtp_username.is_empty() { cfg.email.imap_username.clone() } else { cfg.email.smtp_username.clone() };
    cfg.email.smtp_username = Input::with_theme(theme)
        .with_prompt("SMTP username")
        .default(default_smtp_user)
        .interact_text()
        .unwrap_or_else(|_| cfg.email.smtp_username.clone());

    let smtp_pass: String = Password::with_theme(theme)
        .with_prompt("SMTP password (Enter to keep)")
        .allow_empty_password(currently_enabled)
        .interact()
        .unwrap_or_default();
    if !smtp_pass.is_empty() { cfg.email.smtp_password = smtp_pass; }

    let default_from = if cfg.email.from_address.is_empty() { cfg.email.smtp_username.clone() } else { cfg.email.from_address.clone() };
    cfg.email.from_address = Input::with_theme(theme)
        .with_prompt("From address for replies")
        .default(default_from)
        .interact_text()
        .unwrap_or_else(|_| cfg.email.from_address.clone());

    let raw: String = Input::with_theme(theme)
        .with_prompt("Allowed sender addresses, comma-separated (Enter = allow all)")
        .allow_empty(true)
        .default(cfg.email.allow_from.join(","))
        .interact_text()
        .unwrap_or_default();
    cfg.email.allow_from = raw.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();

    cfg.email.poll_interval_secs = Input::with_theme(theme)
        .with_prompt("Poll interval (seconds)")
        .default(cfg.email.poll_interval_secs)
        .interact_text()
        .unwrap_or(30);

    cfg.email.enabled = true;
    println!("  ✓ Email configured.");
    cfg
}

fn setup_matrix(theme: &ColorfulTheme, mut cfg: PantherConfig) -> PantherConfig {
    let currently_enabled = cfg.matrix.enabled;
    let enable = Confirm::with_theme(theme)
        .with_prompt(if currently_enabled { "Matrix (configured) — reconfigure?" } else { "Enable Matrix/Element?" })
        .default(currently_enabled)
        .interact()
        .unwrap_or(false);

    if !enable {
        if currently_enabled { cfg.matrix.enabled = false; println!("  → Matrix disabled."); }
        return cfg;
    }

    println!("  Get an access token: Element → Settings → Help & About → Advanced");
    println!("  Or register a dedicated bot account.");
    println!();

    cfg.matrix.homeserver = Input::with_theme(theme)
        .with_prompt("Matrix homeserver URL (e.g. https://matrix.org)")
        .default(cfg.matrix.homeserver.clone())
        .interact_text()
        .unwrap_or_else(|_| cfg.matrix.homeserver.clone());

    cfg.matrix.user_id = Input::with_theme(theme)
        .with_prompt("Bot Matrix user ID (e.g. @mybot:matrix.org)")
        .default(cfg.matrix.user_id.clone())
        .interact_text()
        .unwrap_or_else(|_| cfg.matrix.user_id.clone());

    let access_token: String = Password::with_theme(theme)
        .with_prompt("Matrix access token (Enter to keep)")
        .allow_empty_password(currently_enabled)
        .interact()
        .unwrap_or_default();
    if !access_token.is_empty() { cfg.matrix.access_token = access_token; }

    let raw: String = Input::with_theme(theme)
        .with_prompt("Allowed Matrix user IDs, comma-separated (Enter = allow all)")
        .allow_empty(true)
        .default(cfg.matrix.allow_from.join(","))
        .interact_text()
        .unwrap_or_default();
    cfg.matrix.allow_from = raw.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();

    let group_options = &["mention — respond only when @mentioned in rooms", "open — respond to all messages", "allowlist — only allowlisted users in rooms"];
    let current_idx = match cfg.matrix.group_policy.as_str() { "open" => 1, "allowlist" => 2, _ => 0 };
    let policy_idx = Select::with_theme(theme)
        .with_prompt("Room/group policy")
        .items(group_options)
        .default(current_idx)
        .interact()
        .unwrap_or(0);
    cfg.matrix.group_policy = match policy_idx { 1 => "open".to_string(), 2 => "allowlist".to_string(), _ => "mention".to_string() };

    cfg.matrix.enabled = true;
    println!("  ✓ Matrix configured.");
    cfg
}

fn setup_cli(theme: &ColorfulTheme, mut cfg: PantherConfig) -> PantherConfig {
    let enable = Confirm::with_theme(theme)
        .with_prompt("Enable CLI mode (interactive terminal)?")
        .default(cfg.cli.enabled)
        .interact()
        .unwrap_or(false);
    cfg.cli.enabled = enable;
    if enable { println!("  ✓ CLI mode enabled."); }
    cfg
}

fn select_provider(theme: &ColorfulTheme, current: &str) -> usize {
    let items: Vec<String> = PROVIDERS.iter().map(|(_id, label, hint)| {
        if hint.is_empty() { label.to_string() } else { format!("{} [key like {}]", label, hint) }
    }).collect();
    let default_idx = PROVIDERS.iter().position(|(id, _, _)| *id == current).unwrap_or(2);
    Select::with_theme(theme).with_prompt("LLM Provider").items(&items).default(default_idx).interact().unwrap_or(default_idx)
}

fn default_model(provider: &str, current: &str) -> String {
    if !current.is_empty() && current != "gpt-4o" && current != "llama3" { return current.to_string(); }
    DEFAULT_MODELS.iter().find(|(p, _)| *p == provider).map(|(_, m)| m.to_string()).unwrap_or_else(|| current.to_string())
}

fn set_provider_key(provider: &str, key: String, cfg: &mut PantherConfig) {
    match provider {
        "openai"     => cfg.openai_key = key,
        "anthropic"  => cfg.anthropic_key = key,
        "openrouter" => cfg.openrouter_key = key,
        "gemini"     => cfg.gemini_key = key,
        "groq"       => cfg.groq_key = key,
        "mistral"    => cfg.mistral_key = key,
        "deepseek"   => cfg.deepseek_key = key,
        "xai"        => cfg.xai_key = key,
        "together"   => cfg.together_key = key,
        "perplexity" => cfg.perplexity_key = key,
        "cohere"     => cfg.cohere_key = key,
        _ => {}
    }
}

fn prompt_api_key(theme: &ColorfulTheme, provider_label: &str) -> String {
    loop {
        let key = Password::with_theme(theme).with_prompt(format!("{} API Key", provider_label)).interact().unwrap_or_default();
        if !key.is_empty() { return key; }
        println!("  ✗ API key cannot be empty.");
    }
}

fn prompt_mcp_servers(theme: &ColorfulTheme, mut existing: Vec<McpServerConfig>) -> Vec<McpServerConfig> {
    loop {
        let name: String = Input::with_theme(theme).with_prompt("  Server name").interact_text().unwrap_or_default();
        let command: String = Input::with_theme(theme).with_prompt("  Command (e.g. npx)").interact_text().unwrap_or_default();
        let args_raw: String = Input::with_theme(theme).with_prompt("  Args space-separated").allow_empty(true).interact_text().unwrap_or_default();
        if !name.is_empty() && !command.is_empty() {
            let args: Vec<String> = args_raw.split_whitespace().map(|s| s.to_string()).collect();
            existing.push(McpServerConfig { name, command, args });
            println!("  ✓ MCP server added.");
        }
        if !Confirm::with_theme(theme).with_prompt("  Add another?").default(false).interact().unwrap_or(false) { break; }
    }
    existing
}

#[allow(dead_code)]
pub fn register_startup(binary_path: &PathBuf) {
    #[cfg(windows)]
    {
        let startup = std::env::var("APPDATA").ok().map(|a| PathBuf::from(a).join("Microsoft").join("Windows").join("Start Menu").join("Programs").join("Startup"));
        if let Some(dir) = startup {
            let bat = format!("@echo off\nstart \"\" \"{}\"", binary_path.display());
            match std::fs::write(dir.join("panther.bat"), bat) {
                Ok(_) => println!("✓ Panther registered to start on login."),
                Err(e) => println!("Warning: Could not write startup entry: {}", e),
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        let service_dir = dirs::home_dir().map(|h| h.join(".config").join("systemd").join("user"));
        if let Some(dir) = service_dir {
            let _ = std::fs::create_dir_all(&dir);
            let unit = format!("[Unit]\nDescription=Panther AI Agent\nAfter=network.target\n\n[Service]\nExecStart={}\nRestart=on-failure\n\n[Install]\nWantedBy=default.target\n", binary_path.display());
            if std::fs::write(dir.join("panther.service"), unit).is_ok() {
                let _ = std::process::Command::new("systemctl").args(["--user", "daemon-reload"]).status();
                let _ = std::process::Command::new("systemctl").args(["--user", "enable", "panther"]).status();
                println!("✓ Panther registered as a systemd user service.");
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        let plist_dir = dirs::home_dir().map(|h| h.join("Library").join("LaunchAgents"));
        if let Some(dir) = plist_dir {
            let _ = std::fs::create_dir_all(&dir);
            let plist = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key><string>ai.panther.daemon</string>\n  <key>ProgramArguments</key><array><string>{}</string></array>\n  <key>RunAtLoad</key><true/>\n  <key>KeepAlive</key><true/>\n</dict>\n</plist>", binary_path.display());
            if std::fs::write(dir.join("ai.panther.daemon.plist"), plist).is_ok() {
                let _ = std::process::Command::new("launchctl").args(["load", &dir.join("ai.panther.daemon.plist").to_string_lossy()]).status();
                println!("✓ Panther registered as a Launch Agent.");
            }
        }
    }
}
