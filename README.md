<div align="center">

<!-- PLACEHOLDER: .github/assets/banner.png (recommended 1280x320px) -->
<img src=".github/assets/banner.png" alt="Panther" width="100%" />

<br/>
<br/>

**A self-hosted AI agent daemon that runs on your machine and talks back through the apps you already use.**

<br/>

![Rust](https://img.shields.io/badge/Rust-2021_Edition-orange?logo=rust)
![License](https://img.shields.io/badge/license-MIT-blue)
![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-lightgrey)

[Features](#features) · [Architecture](#architecture) · [Providers](#llm-providers) · [Channels](#messaging-channels) · [Tools](#built-in-tools) · [Install](#installation) · [Configuration](#configuration) · [Security](#security)

</div>

---

## What is Panther?

Panther is a Rust daemon that sits on your computer and acts as your personal AI assistant, reachable from any messaging app you already use. Send it a message on Telegram, Discord, Slack, or even email, and it reasons, executes tools, and responds from your own hardware.

It can run entirely on a local model through Ollama (zero data leaves your machine) or connect to any of twelve cloud providers when you need more capability. The provider is a single config value; the rest of the system does not change.

<br/>

## Features

| Category | Capabilities |
|---|---|
| **LLM Providers** | Ollama, OpenAI, Anthropic, OpenRouter, Gemini, Groq, Mistral, DeepSeek, xAI, TogetherAI, Perplexity, Cohere |
| **Channels** | Telegram, Discord, Slack, Email (IMAP/SMTP), Matrix, Local CLI |
| **File Tools** | Read, write, edit, list directory |
| **System Tools** | Shell execution, clipboard, system info |
| **Media Tools** | Screenshots, webcam photos, audio recording, screen recording |
| **Web Tools** | Brave Search API, URL fetch with HTML stripping |
| **Scheduling** | Cron expressions, fixed intervals, one-time timestamps |
| **Agent Tools** | Subagent spawning, MCP server integration, custom skills |
| **Memory** | Persistent conversation history, user profile, session consolidation |
| **Transcription** | Voice message transcription via Groq Whisper |

<br/>

## Screenshots

> Replace each image with a smartphone mockup screenshot. See [`.github/assets/PLACEHOLDER_GUIDE.md`](.github/assets/PLACEHOLDER_GUIDE.md) for filenames and dimensions.

<div align="center">
<table>
<tr>
<td align="center">
<img src=".github/assets/one.gif" width="200" alt="Chat demo" />
<br/><sub>Conversational chat</sub>
</td>
<td align="center">
<img src=".github/assets/two.gif" width="200" alt="Screenshot demo" />
<br/><sub>Screen capture and send</sub>
</td>
<td align="center">
<img src=".github/assets/three.gif" width="200" alt="Exec demo" />
<br/><sub>Shell command output</sub>
</td>
<td align="center">
<img src=".github/assets/four.gif" width="200" alt="Cron demo" />
<br/><sub>Task scheduling</sub>
</td>
</tr>
<tr>
<td align="center">
<img src=".github/assets/five.gif" width="200" alt="Web search" />
<br/><sub>webcam capture</sub>
</td>
<td align="center">
<img src=".github/assets/six.gif" width="200" alt="Subagent" />
<br/><sub>Background subagent</sub>
</td>
<td align="center">
<img src=".github/assets/seven.gif" width="200" alt="Local mode" />
<br/><sub>Fully local Ollama mode</sub>
</td>
<td align="center">
<img src=".github/assets/eight.gif" width="200" alt="Multi-channel" />
<br/><sub>Multi-channel access</sub>
</td>
</tr>
</table>
</div>

<br/>

---

## Architecture

Panther is a Cargo workspace of nine focused crates. Each crate owns a single responsibility.

```
panther/
└── crates/
    ├── daemon/          # Entry point: `panther` binary + `panther-install` wizard
    ├── agent/           # Core reasoning loop, sessions, tool dispatch, subagents
    ├── bot/             # Channel adapters (Telegram, Discord, Slack, Email, Matrix, CLI)
    ├── providers/       # LLM provider implementations + unified router
    ├── memory/          # Persistent storage: profile, history, skills
    ├── mcp/             # Model Context Protocol client (stdio JSON-RPC 2.0)
    ├── skill-runner/    # Sandboxed subprocess runner for custom skill scripts
    ├── context-engine/  # Context assembly layer (extensible)
    └── shared/          # Common types, MessageBus, Channel trait, errors
```

### System Flow

How a message travels from your phone to a response:

```
  User (Telegram / Discord / Slack / Email / Matrix / CLI)
         |
         | platform API / IMAP poll / WebSocket
         v
  +--------------+
  |   Bot Crate  |  Normalises raw platform events into InboundMessage structs.
  |  (Adapters)  |  Applies the per-channel allow_from whitelist.
  |              |  Discards unknown senders silently.
  +--------------+
         |
         | InboundMessage pushed onto MessageBus (tokio mpsc channel)
         v
  +--------------+
  |  Agent Crate |  Session identified by channel + chat_id.
  |  (Dispatcher)|  Per-session Semaphore serialises concurrent messages.
  |              |  History loaded, profile injected, context assembled.
  +--------------+
         |
         | LLMRequest: system prompt + history + all tool definitions
         v
  +-----------------+
  | Provider Router |  Routes to active provider (Ollama, OpenAI, etc.).
  |                 |  All twelve providers share the same trait interface.
  +-----------------+
         |
         | LLMResponse: text (final) or ToolCall list (continue loop)
         v
  +--------------+
  | Tool Loop    |  Executes requested tools via ToolRegistry.
  |              |  Results appended to message history.
  |              |  Repeats until text response or max_iterations reached.
  +--------------+
         |
         | OutboundMessage pushed onto output bus
         v
  +--------------------+
  | OutboundDispatcher |  Routes reply back to the originating channel.
  |                    |  Handles message chunking, file uploads, media.
  +--------------------+
         |
         v
  User receives response
```

### The Agent Loop

The reasoning loop (`crates/agent/src/loop.rs`) is the core of the system. Each turn works like this:

```
  Iteration 0
  +-----------+
  | Build     |  System prompt (OS, persona, profile, datetime)
  | context   |  + last N messages from history (memory_window)
  |           |  + definitions for all tools (built-in + MCP)
  +-----------+
       |
       v
  +-----------+
  | LLM call  |  POST to active provider endpoint.
  |           |  Timeout: 300s local, varies for cloud.
  +-----------+
       |
       +-- No tool calls --> emit final text, exit loop
       |
       v
  +-----------+
  | Tool      |  Tools run via ToolRegistry.
  | execution |  Results truncated to tool_result_truncation chars.
  |           |  Appended as `tool` role messages in context.
  +-----------+
       |
       | If send_progress = true, intermediate message sent to user now
       v
  Iteration 1 ... up to max_iterations (default: 40)

  If cap reached with no final text, agent sends an error and ends the turn.
```

Tool results go back into the conversation as `tool` role messages. The model sees its own prior calls and their outcomes, enabling multi-step chains: search the web, read a URL, write a file, confirm to the user, all in a single turn.

### Message Bus

The bot and agent layers are fully decoupled via a typed `MessageBus` built on Tokio MPSC channels:

```
  InboundMessage  { channel, sender_id, chat_id, content, media_path, image_b64 }
  OutboundMessage { channel, chat_id, content, file_path, is_progress, is_tool_hint }
```

The agent never imports the bot crate. The `channel` field on outbound messages is sufficient for the dispatcher to route replies correctly.

### Provider Router

All twelve providers implement a single async trait:

```rust
#[async_trait]
pub trait ProviderInterface: Send + Sync {
    async fn chat(&self, request: LLMRequest) -> PantherResult<LLMResponse>;
}
```

`ProviderRouter` holds one instance of every provider and dispatches to whichever is set as `active`. Switching providers is a config change and restart. Ollama uses the same OpenAI-compatible JSON format as the cloud adapters, so the same `oai_common` serialisation layer covers both.

### Sessions and Concurrency

Each unique `channel:chat_id` pair is an isolated session with its own history and lock:

```
  Telegram user A  ->  Session "telegram:111"  ->  own history, own semaphore
  Telegram user B  ->  Session "telegram:222"  ->  own history, own semaphore
  Discord user C   ->  Session "discord:333"   ->  own history, own semaphore
```

A `Semaphore(1)` per session serialises messages from the same chat. Different sessions run concurrently on the Tokio runtime. The daemon handles many users simultaneously with no configuration needed.

### Subagent Architecture

When the `spawn` tool is called, a full independent agent instance is created in a new Tokio task:

```
  Primary Agent
  |
  +-- spawn("task description") -------->  Subagent (Tokio task)
  |                                        - own ToolRegistry
  |   Primary continues normally           - own reasoning loop
  |                                        - shared ProviderRouter
  |                                        - no spawn / message tools
  |                                        |
  |<-- result sent to originating chat  ---+
```

Multiple subagents can run in parallel. Each is tracked by session so its handle can be cleaned up when done.

<br/>

---

## LLM Providers

| Provider | Config key | Notes |
|---|---|---|
| **Ollama** | `ollama_host`, `ollama_model` | Local inference, default provider |
| **OpenAI** | `openai_key` | GPT-4o and all GPT variants |
| **Anthropic** | `anthropic_key` | Claude family |
| **OpenRouter** | `openrouter_key` | Single key for many models |
| **Gemini** | `gemini_key` | Google Gemini family |
| **Groq** | `groq_key` | Fast inference + Whisper transcription |
| **Mistral** | `mistral_key` | Mistral family |
| **DeepSeek** | `deepseek_key` | DeepSeek models |
| **xAI** | `xai_key` | Grok family |
| **TogetherAI** | `together_key` | Hosted open-weight models |
| **Perplexity** | `perplexity_key` | Online models with built-in search |
| **Cohere** | `cohere_key` | Command family |

`cloud_model` in config is the model string sent to all non-Ollama providers. Set it to the specific model you want (e.g. `claude-3-5-sonnet-20241022`, `gpt-4o`, `gemini-2.0-flash`).

<br/>

---

## Messaging Channels

### Telegram
The primary channel. Supports text, file uploads, voice messages (auto-transcribed), and photo attachments. Long responses are chunked automatically at the 4096-character limit.

### Discord
Uses Serenity via WebSocket gateway. Responds in guild channels or DMs where the bot token has access.

### Slack
Connects via Socket Mode using an app-level token. No public endpoint or webhook URL required.

### Email
Polls an IMAP mailbox (default every 30 seconds). Replies via SMTP with TLS. HTML emails are stripped to plain text before the agent sees them.

### Matrix
Works with any homeserver using an access token. `group_policy = "mention"` requires the bot to be addressed directly; `"all"` replies to every message in a room.

### CLI
An interactive terminal REPL. No external accounts needed. Useful for local testing or terminal-native workflows.

<br/>

---

## Built-in Tools

### Shell Execution
Runs commands via `sh -c` on Linux/macOS and PowerShell on Windows. A static blocklist covers the most destructive patterns (`rm -rf`, `mkfs`, `dd if=`, fork bombs, disk format commands). Configurable timeout, default 30 seconds.

### Filesystem
`read_file`, `write_file`, `edit_file`, `list_dir`. The edit tool does a safe single-occurrence string replace and refuses if the target appears more than once, preventing ambiguous edits. Home-relative `~/` paths expand correctly on all platforms.

### Web Search
Calls the Brave Search API and returns titles, URLs, and snippets. Requires `brave_api_key` in config.

### Web Fetch
Fetches a URL and strips HTML to plain readable text. Scripts, styles, and tags excluded. Returns up to 8000 characters by default.

### Media Capture

Capture works on all platforms through automatic fallback chains. Each method is tried in order; the first that succeeds is used.

**Screenshot**

| Platform | Fallback chain |
|---|---|
| Windows | ffmpeg gdigrab > PowerShell System.Windows.Forms > GDI32 P/Invoke |
| macOS | `screencapture` (native) |
| Linux | gnome-screenshot > scrot > spectacle > flameshot > ImageMagick import > ffmpeg x11grab > grim/wayshot |

**Webcam**

| Platform | Fallback chain |
|---|---|
| Windows | ffmpeg dshow > Python OpenCV > VLC > WinRT MediaCapture |
| macOS | imagesnap > ffmpeg avfoundation |
| Linux | ffmpeg v4l2 > streamer |

**Audio**

| Platform | Fallback chain |
|---|---|
| Windows | MCI/winmm.dll (native, no deps required) > ffmpeg dshow > ffmpeg wasapi > Python sounddevice |
| macOS | ffmpeg avfoundation > rec (sox) |
| Linux | ffmpeg pulse > ffmpeg pipewire > ffmpeg alsa > arecord > parecord |

**Screen Recording**

| Platform | Fallback chain |
|---|---|
| Windows | ffmpeg gdigrab > PowerShell frame capture + ffmpeg/opencv assembly |
| macOS | ffmpeg avfoundation |
| Linux | ffmpeg x11grab > wf-recorder/wl-screenrec > recordmydesktop |

Captured files are automatically sent back to the chat as file uploads.

### Clipboard
Read from or write to the system clipboard. `pbpaste`/`pbcopy` on macOS, PowerShell `Get-Clipboard`/`Set-Clipboard` on Windows, `xclip`/`xsel`/`wl-paste` on Linux.

### System Information
Reports CPU, RAM, disk, battery, uptime, hostname, network interfaces, running processes, open windows, and display info. Queryable by category or all at once.

### Cron Scheduling
Three schedule types: an exact timestamp (`at`), a repeating interval in milliseconds (`every`), and a 5-field cron expression with optional timezone (`cron`). Jobs persist across restarts. One-off jobs can auto-delete after running.

### Spawn
Delegates a task to an independent background subagent. The primary agent is not blocked while it runs.

### MCP
Routes tool calls to external MCP server processes over stdio JSON-RPC 2.0. Tools from all configured servers are listed at startup alongside built-ins.

### Custom Skills
Any executable in `~/.panther/skills/` is registered as a callable tool. Input via `PANTHER_INPUT` env var, output via stdout, 30-second timeout.

<br/>

---

## Security

**Access control.** Every channel has an `allow_from` list. Messages from senders not on the list are discarded before reaching the agent. An empty list accepts everyone; use only for personal, isolated setups.

**Command blocking.** The exec tool has a static blocklist for unrecoverable operations. This is defence-in-depth, not a sandbox. The agent runs under your user account. Do not run Panther as root or with elevated privileges.

**Credentials.** Keys live in `~/.panther/config.toml`. Set permissions to `600` on Linux/macOS. Keys are only transmitted to the corresponding provider endpoint.

**Network exposure.** Panther opens no listening ports. It connects outward only. There is no inbound attack surface.

**Local mode.** With Ollama and the CLI channel, no data leaves the machine at any layer.

**Honest note.** Giving an LLM shell access carries real risk if adversarial content enters through fetched web pages or tool results. The blocklist reduces the worst-case outcomes but is not exhaustive. Run Panther under a least-privilege account.

<br/>

---

## Installation

### Prerequisites

Install Rust via [rustup](https://rustup.rs/):

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Minimum: Rust 2021 edition (1.70+ recommended).

---

### Linux

```sh
# 1. Dependencies (Debian/Ubuntu)
sudo apt install build-essential pkg-config libssl-dev

# Fedora/RHEL
sudo dnf install gcc pkg-config openssl-devel

# Arch
sudo pacman -S base-devel pkg-config openssl

# 2. Build
git clone https://github.com/your-username/panther.git
cd panther
cargo build --release

# 3. Install (optional)
sudo cp target/release/panther /usr/local/bin/
sudo cp target/release/panther-install /usr/local/bin/

# 4. Optional: media + clipboard tools
sudo apt install ffmpeg scrot xclip wl-clipboard
```

---

### macOS

```sh
# 1. Xcode tools
xcode-select --install

# 2. OpenSSL
brew install openssl

# If build cannot find it:
export PKG_CONFIG_PATH="$(brew --prefix openssl)/lib/pkgconfig"

# 3. Build
git clone https://github.com/your-username/panther.git
cd panther
cargo build --release

# 4. Install (optional)
cp target/release/panther /usr/local/bin/
cp target/release/panther-install /usr/local/bin/

# 5. Optional: media tools
brew install ffmpeg imagesnap
# screencapture is built into macOS, no install needed
```

---

### Windows

```powershell
# 1. Install Rust
# Download rustup-init.exe from https://win.rustup.rs/
# Accept the default msvc toolchain.
# Install Visual Studio Build Tools with C++ workload when prompted.

# 2. Build
git clone https://github.com/your-username/panther.git
cd panther
cargo build --release

# 3. Copy binaries to a directory in your PATH (optional)
# target\release\panther.exe
# target\release\panther-install.exe

# 4. Optional: ffmpeg for enhanced media capture
winget install Gyan.FFmpeg
# Without ffmpeg, Panther falls back to native PowerShell/MCI backends.
```

<br/>

---

## Configuration

Config is stored at `~/.panther/config.toml`. Run the interactive wizard to create it:

```sh
panther-install
```

### Full annotated config

```toml
# ── LLM Provider ─────────────────────────────────────────────────────────────
# Options: ollama, openai, anthropic, openrouter, gemini,
#          groq, mistral, deepseek, xai, together, perplexity, cohere
active_provider = "ollama"

ollama_host  = "http://localhost:11434"
ollama_model = "llama3"
cloud_model  = "gpt-4o"    # Used by all non-Ollama providers

openai_key      = ""
anthropic_key   = ""
openrouter_key  = ""
gemini_key      = ""
groq_key        = ""
mistral_key     = ""
deepseek_key    = ""
xai_key         = ""
together_key    = ""
perplexity_key  = ""
cohere_key      = ""

# ── Web Search ────────────────────────────────────────────────────────────────
# Get a key at https://brave.com/search/api/
brave_api_key = ""

# ── Telegram ──────────────────────────────────────────────────────────────────
telegram_token      = ""
telegram_allow_from = ["123456789"]    # Numeric Telegram user IDs

# ── Discord ───────────────────────────────────────────────────────────────────
discord_token      = ""
discord_allow_from = []

# ── Slack ─────────────────────────────────────────────────────────────────────
[slack]
enabled    = false
app_token  = ""    # xapp-1-...
bot_token  = ""    # xoxb-...
allow_from = []

# ── Email ─────────────────────────────────────────────────────────────────────
[email]
enabled             = false
imap_host           = "imap.example.com"
imap_port           = 993
imap_username       = ""
imap_password       = ""
imap_mailbox        = "INBOX"
smtp_host           = "smtp.example.com"
smtp_port           = 587
smtp_username       = ""
smtp_password       = ""
from_address        = "panther@example.com"
allow_from          = ["you@example.com"]
poll_interval_secs  = 30
max_body_chars      = 12000

# ── Matrix ────────────────────────────────────────────────────────────────────
[matrix]
enabled      = false
homeserver   = "https://matrix.org"
access_token = ""
user_id      = ""
allow_from   = []
group_policy = "mention"    # "mention" or "all"

# ── CLI ───────────────────────────────────────────────────────────────────────
[cli]
enabled = false

# ── Agent Behaviour ───────────────────────────────────────────────────────────
max_iterations          = 40      # Tool-use loop iteration cap per turn
max_tokens              = 8096
temperature             = 0.1
memory_window           = 100     # Messages kept in context
tool_result_truncation  = 500     # Max chars per tool result in context
exec_timeout_secs       = 30
exec_path_append        = ""      # Extra PATH entries for the exec tool
heartbeat_interval_secs = 1800
send_progress           = false   # Send live progress during tool use
send_tool_hints         = false   # Annotate responses with tool names used

# ── Audio Transcription ───────────────────────────────────────────────────────
groq_transcription_key = ""
transcription_model    = "whisper-large-v3"

# ── MCP Servers ───────────────────────────────────────────────────────────────
[[mcp_servers]]
name    = "filesystem"
command = "npx"
args    = ["-y", "@modelcontextprotocol/server-filesystem", "/home/user/docs"]
```

<br/>

---

## Running Panther

```sh
# First-time setup wizard
panther-install

# Start the daemon
panther
```

Panther prints `Panther is online.` and begins listening. It runs in the foreground.

**Run as a background service:**

<details>
<summary>Linux (systemd user service)</summary>

```ini
# ~/.config/systemd/user/panther.service
[Unit]
Description=Panther AI Agent

[Service]
ExecStart=/usr/local/bin/panther
Restart=on-failure

[Install]
WantedBy=default.target
```

```sh
systemctl --user enable --now panther
```
</details>

<details>
<summary>macOS (launchd)</summary>

```xml
<!-- ~/Library/LaunchAgents/com.panther.agent.plist -->
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.panther.agent</string>
  <key>ProgramArguments</key>
  <array><string>/usr/local/bin/panther</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
</dict>
</plist>
```

```sh
launchctl load ~/Library/LaunchAgents/com.panther.agent.plist
```
</details>

<details>
<summary>Windows (Task Scheduler)</summary>

```powershell
$action  = New-ScheduledTaskAction -Execute "C:\Program Files\Panther\panther.exe"
$trigger = New-ScheduledTaskTrigger -AtLogOn
Register-ScheduledTask -TaskName "Panther" -Action $action -Trigger $trigger -RunLevel Highest
```
</details>

<br/>

---

## Local-Only Mode

To run with absolutely no external network calls:

```sh
# 1. Install Ollama and pull a model
ollama pull llama3
```

```toml
# 2. Config
active_provider = "ollama"
ollama_host     = "http://localhost:11434"
ollama_model    = "llama3"
# Leave all cloud keys and brave_api_key empty

[cli]
enabled = true
```

Tool calling in local mode depends on the model. Reliable choices: `llama3`, `mistral`, `qwen2.5`. Smaller models may produce malformed tool calls; `max_iterations` acts as a circuit breaker.

<br/>

---

## MCP Server Integration

Any MCP-compatible tool server can be connected via config:

```toml
[[mcp_servers]]
name    = "my-tools"
command = "python"
args    = ["/path/to/server.py"]
```

Panther spawns the server as a child process and communicates via stdio JSON-RPC 2.0. At startup it calls `tools/list` on each server and registers the results alongside built-ins. On Windows, servers launch via `cmd /C` for correct environment resolution.

<br/>

---

## Custom Skills

Place any executable script in `~/.panther/skills/`. Panther registers it as a callable tool.

- Input passed via `PANTHER_INPUT` environment variable
- Output read from stdout
- Hard timeout: 30 seconds
- Any language that produces an executable file works

```python
#!/usr/bin/env python3
# ~/.panther/skills/weather.py
import os, requests
city = os.environ.get("PANTHER_INPUT", "London")
print(requests.get(f"https://wttr.in/{city}?format=3").text)
```

```sh
chmod +x ~/.panther/skills/weather.py
```

<br/>

---

## Memory and Persistence

```
~/.panther/
├── config.toml               # Main config (atomic writes via tmp-then-rename)
├── profile/                  # User profile: name, preferences, projects, style
├── history/                  # Per-session conversation logs (JSON)
├── chats/
│   └── known_chats.json      # channel:chat_id to session ID map
└── skills/                   # Custom skill scripts
```

The user profile is injected into the system prompt on every turn, so the agent maintains consistent knowledge of your preferences and projects across sessions and restarts.

When a session's history grows large, the agent runs a consolidation pass that summarises older exchanges into a compact form, keeping context size manageable without losing important facts.

<br/>

---

## Performance

| Metric | Typical value |
|---|---|
| Startup time (no MCP) | under 1 second |
| Idle memory footprint | 20-60 MB |
| Response latency (Ollama + GPU) | 2-10 seconds |
| Response latency (cloud) | varies by provider and model |
| Concurrent sessions | unlimited (one Tokio task per session) |
| Built-in tool overhead | negligible (native async Rust) |
| MCP / skill tool overhead | 100-500 ms (subprocess IPC) |

Set `send_progress = true` to receive intermediate status messages during long tool chains so the chat does not go silent while work is happening.

<br/>

---

## Contributing

Open an issue before submitting a pull request for significant changes.

**Adding a provider:** implement `ProviderInterface` in `crates/providers/src/`, add the variant to `LLMProvider` in `crates/shared/src/types.rs`, register it in `ProviderRouter`, add the config field to `PantherConfig`.

**Adding a channel:** implement the `Channel` trait from `crates/shared/src/channel.rs`, add a config struct, wire it into `crates/daemon/src/bootstrap.rs`.

Format with `cargo fmt`. Lint with `cargo clippy`.

---

## License

MIT. See [LICENSE](LICENSE).

---

<div align="center">
<sub>Built in Rust. Runs on your machine. Answers to you.</sub>
</div>
