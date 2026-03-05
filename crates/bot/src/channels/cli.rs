use std::path::PathBuf;
use std::pin::Pin;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use std::io::{self, Write};

use crossterm::{
    execute, terminal,
    style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor},
};
use shared::bus::{InboundMessage, MessageBus, OutboundMessage};
use shared::channel::{Channel, FileKind};
use shared::errors::{PantherError, PantherResult};
use tokio::sync::mpsc;

pub const CLI_CHANNEL: &str = "cli";
const CLI_CHAT_ID: &str = "terminal";

const RESET:   &str = "\x1b[0m";
const BOLD:    &str = "\x1b[1m";

const S1: &str = "\x1b[38;5;240m";
const S2: &str = "\x1b[38;5;244m";
const S3: &str = "\x1b[38;5;248m";
const S4: &str = "\x1b[38;5;252m";

const CYAN:    &str = "\x1b[96m";
const MAGENTA: &str = "\x1b[95m";
const RED:     &str = "\x1b[91m";
const GREY:    &str = "\x1b[90m";

const AMBER:       Color = Color::Rgb { r: 245, g: 166, b: 35  };
const AMBER_DIM:   Color = Color::Rgb { r: 120, g: 78,  b: 10  };
const SLATE_LIGHT: Color = Color::Rgb { r: 148, g: 163, b: 184 };
const ICE:         Color = Color::Rgb { r: 186, g: 230, b: 253 };
const SNOW:        Color = Color::Rgb { r: 226, g: 232, b: 240 };
const CODE_COLOR:  Color = Color::Rgb { r: 129, g: 212, b: 250 };
const BORDER:      Color = Color::Rgb { r: 51,  g: 65,  b: 85  };
const DIM_BORDER:  Color = Color::Rgb { r: 38,  g: 48,  b: 62  };

struct Spinner {
    handle: tokio::task::JoinHandle<()>,
    stop:   Arc<std::sync::atomic::AtomicBool>,
}

fn term_width() -> usize {
    terminal::size().map(|(w, _)| w as usize).unwrap_or(88).min(120)
}

fn hide_cursor() { print!("\x1b[?25l"); let _ = io::stdout().flush(); }
fn show_cursor() { print!("\x1b[?25h"); let _ = io::stdout().flush(); }
fn clear_line()  { print!("\r\x1b[2K"); let _ = io::stdout().flush(); }

fn start_spinner(label: &str) -> Spinner {
    let stop   = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag   = Arc::clone(&stop);
    let label  = label.to_string();
    let handle = tokio::spawn(async move {
        let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let mut i  = 0usize;
        hide_cursor();
        while !flag.load(std::sync::atomic::Ordering::Relaxed) {
            let frame = frames[i % frames.len()];
            print!("\r  {}{}{} {}{}{}   ", CYAN, frame, RESET, GREY, label, RESET);
            let _ = io::stdout().flush();
            tokio::time::sleep(Duration::from_millis(80)).await;
            i += 1;
        }
    });
    Spinner { handle, stop }
}

fn stop_spinner(s: Spinner) {
    s.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    s.handle.abort();
    clear_line();
    show_cursor();
}

type InputGate = Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>;

pub struct CliChannel {
    bus:         MessageBus,
    response_tx: mpsc::Sender<OutboundMessage>,
    response_rx: Arc<tokio::sync::Mutex<mpsc::Receiver<OutboundMessage>>>,
}

impl CliChannel {
    pub fn new(bus: MessageBus) -> Self {
        let (tx, rx) = mpsc::channel(64);
        Self { bus, response_tx: tx, response_rx: Arc::new(tokio::sync::Mutex::new(rx)) }
    }

    fn session_key() -> String {
        format!("{}:{}", CLI_CHANNEL, CLI_CHAT_ID)
    }

    pub async fn run_loop(self: Arc<Self>) {
        print_banner();

        let (line_tx, mut line_rx) = mpsc::channel::<String>(32);
        let bus = self.bus.clone();

        let gate: InputGate = Arc::new((std::sync::Mutex::new(true), std::sync::Condvar::new()));
        let gate_thread = Arc::clone(&gate);

        std::thread::spawn(move || {
            let mut rl = match rustyline::DefaultEditor::new() {
                Ok(r)  => r,
                Err(e) => { eprintln!("[panther:cli] readline init failed: {}", e); return; }
            };
            let history_path = dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".panther")
                .join(".cli_history");
            let _ = rl.load_history(&history_path);

            loop {
                {
                    let (lock, cvar) = &*gate_thread;
                    let mut ready = lock.lock().unwrap();
                    while !*ready {
                        ready = cvar.wait(ready).unwrap();
                    }
                    *ready = false;
                }

                // Wrap every ANSI sequence in \x01…\x02 so rustyline treats them as
                // zero-width when calculating cursor position.  Without this the
                // escape-byte count is added to the visible prompt width, pushing
                // every subsequent keystroke to a phantom column far from the left.
                let prompt = format!(
                    "\x01{}{}\x02 ❯ \x01{}\x02",
                    BOLD, CYAN, RESET
                );
                match rl.readline(&prompt) {
                    Ok(line) => {
                        let t = line.trim().to_string();
                        if t.is_empty() {
                            let (lock, cvar) = &*gate_thread;
                            *lock.lock().unwrap() = true;
                            cvar.notify_one();
                            continue;
                        }
                        let _ = rl.add_history_entry(&t);
                        if line_tx.blocking_send(t).is_err() { break; }
                    }
                    Err(rustyline::error::ReadlineError::Interrupted) => {
                        print_ctrl_c_hint();
                        let (lock, cvar) = &*gate_thread;
                        *lock.lock().unwrap() = true;
                        cvar.notify_one();
                        continue;
                    }
                    Err(rustyline::error::ReadlineError::Eof) => {
                        let _ = line_tx.blocking_send("/exit".to_string());
                        break;
                    }
                    Err(e) => {
                        eprintln!("{}✗ readline error: {}{}", RED, e, RESET);
                        break;
                    }
                }
            }
            let _ = std::fs::create_dir_all(
                history_path.parent().unwrap_or(std::path::Path::new("."))
            );
            let _ = rl.save_history(&history_path);
        });

        let mut rx = self.response_rx.lock().await;
        let mut spinner: Option<Spinner> = None;
        let mut tool_count = 0usize;
        let mut started_at = std::time::Instant::now();

        loop {
            tokio::select! {
                Some(line) = line_rx.recv() => {
                    if line == "/exit" || line == "/quit" {
                        print_goodbye();
                        std::process::exit(0);
                    }

                    started_at = std::time::Instant::now();
                    tool_count = 0;
                    println!();
                    spinner = Some(start_spinner("thinking"));

                    let inbound = InboundMessage {
                        channel:              CLI_CHANNEL.to_string(),
                        sender_id:            "user".to_string(),
                        chat_id:              CLI_CHAT_ID.to_string(),
                        content:              line,
                        media_path:           None,
                        image_b64:            None,
                        session_key_override: Some(Self::session_key()),
                    };

                    bus.publish_inbound(inbound).await;
                }

                Some(msg) = rx.recv() => {
                    if let Some(s) = spinner.take() {
                        stop_spinner(s);
                    }
                    if msg.is_tool_hint {
                        tool_count += 1;
                        print_tool_call(&msg.content);
                        spinner = Some(start_spinner("working"));
                    } else if msg.is_progress {
                        print_status(&msg.content);
                        spinner = Some(start_spinner("working"));
                    } else {
                        let elapsed = started_at.elapsed();
                        stream_response(&msg.content, elapsed, tool_count).await;
                        let (lock, cvar) = &*gate;
                        *lock.lock().unwrap() = true;
                        cvar.notify_one();
                    }
                }
            }
        }
    }
}

impl Channel for CliChannel {
    fn name(&self) -> &str { CLI_CHANNEL }

    fn send<'a>(
        &'a self,
        _chat_id: &'a str,
        text: &'a str,
    ) -> Pin<Box<dyn Future<Output = PantherResult<()>> + Send + 'a>> {
        let text = text.to_string();
        let tx   = self.response_tx.clone();
        Box::pin(async move {
            tx.send(OutboundMessage::text(CLI_CHANNEL, CLI_CHAT_ID, text))
                .await
                .map_err(|_| PantherError::ChannelError("CLI response channel closed".into()))?;
            Ok(())
        })
    }

    fn send_file<'a>(
        &'a self,
        _chat_id: &'a str,
        path: &'a PathBuf,
        _kind: FileKind,
    ) -> Pin<Box<dyn Future<Output = PantherResult<()>> + Send + 'a>> {
        let display = path.display().to_string();
        let tx      = self.response_tx.clone();
        Box::pin(async move {
            tx.send(OutboundMessage::text(
                CLI_CHANNEL, CLI_CHAT_ID,
                format!("file  →  {}", display),
            ))
            .await
            .map_err(|_| PantherError::ChannelError("CLI response channel closed".into()))?;
            Ok(())
        })
    }
}

async fn stream_response(text: &str, elapsed: std::time::Duration, tool_count: usize) {
    let w         = term_width();
    let indent    = 2usize;
    let available = w.saturating_sub(indent + 2);

    let info = format_elapsed(elapsed, tool_count);
    println!("  {}panther{}  {}{}{}", MAGENTA, RESET, S1, info, RESET);
    println!();

    render_markdown_streamed(text, indent, available).await;

    println!();
}

async fn render_markdown_streamed(text: &str, indent: usize, available: usize) {
    let lines: Vec<&str>  = text.lines().collect();
    let mut in_code_block = false;
    let mut lang          = String::new();
    let ind               = " ".repeat(indent);

    for raw in &lines {
        let line = *raw;

        if line.trim_start().starts_with("```") {
            if !in_code_block {
                in_code_block = true;
                lang = line.trim_start().trim_start_matches('`').trim().to_string();
                let label = if lang.is_empty() {
                    String::new()
                } else {
                    format!(" {} ", lang)
                };
                let fill = available.saturating_sub(label.len() + 2);
                let _ = execute!(
                    std::io::stdout(),
                    SetForegroundColor(BORDER),
                    Print(format!("{}╭{}{}\n", ind, label, "─".repeat(fill))),
                    ResetColor,
                );
            } else {
                in_code_block = false;
                lang.clear();
                let _ = execute!(
                    std::io::stdout(),
                    SetForegroundColor(DIM_BORDER),
                    Print(format!("{}╰{}\n", ind, "─".repeat(available))),
                    ResetColor,
                );
                println!();
            }
            tokio::time::sleep(Duration::from_millis(8)).await;
            continue;
        }

        if in_code_block {
            let _ = execute!(
                std::io::stdout(),
                SetForegroundColor(BORDER),      Print(format!("{}│ ", ind)),
                SetForegroundColor(CODE_COLOR),  Print(format!("{}\n", line)),
                ResetColor,
            );
            let _ = io::stdout().flush();
            tokio::time::sleep(Duration::from_millis(5)).await;
            continue;
        }

        if line.trim().is_empty() {
            println!();
            tokio::time::sleep(Duration::from_millis(4)).await;
            continue;
        }

        if let Some(h) = line.strip_prefix("### ") {
            let _ = execute!(
                std::io::stdout(),
                SetForegroundColor(ICE), SetAttribute(Attribute::Bold),
                Print(format!("{}{}\n", ind, h)),
                ResetColor, SetAttribute(Attribute::Reset),
            );
            tokio::time::sleep(Duration::from_millis(12)).await;
            continue;
        }
        if let Some(h) = line.strip_prefix("## ") {
            println!();
            let _ = execute!(
                std::io::stdout(),
                SetForegroundColor(ICE), SetAttribute(Attribute::Bold),
                Print(format!("{}{}\n", ind, h)),
                ResetColor, SetAttribute(Attribute::Reset),
            );
            let _ = execute!(
                std::io::stdout(),
                SetForegroundColor(BORDER),
                Print(format!("{}{}\n", ind, "─".repeat(h.len().min(available)))),
                ResetColor,
            );
            tokio::time::sleep(Duration::from_millis(12)).await;
            continue;
        }
        if let Some(h) = line.strip_prefix("# ") {
            println!();
            let _ = execute!(
                std::io::stdout(),
                SetForegroundColor(ICE), SetAttribute(Attribute::Bold),
                Print(format!("{}{}\n", ind, h.to_uppercase())),
                ResetColor, SetAttribute(Attribute::Reset),
            );
            let _ = execute!(
                std::io::stdout(),
                SetForegroundColor(AMBER_DIM),
                Print(format!("{}{}\n", ind, "═".repeat(h.len().min(available)))),
                ResetColor,
            );
            tokio::time::sleep(Duration::from_millis(12)).await;
            continue;
        }

        if line.starts_with("- ") || line.starts_with("* ") {
            let content = &line[2..];
            let _ = execute!(
                std::io::stdout(),
                SetForegroundColor(AMBER),
                Print(format!("{}  •  ", ind)),
                ResetColor,
            );
            print_wrapped_inline(content, indent + 5, available.saturating_sub(5));
            tokio::time::sleep(Duration::from_millis(10)).await;
            continue;
        }

        if let Some((num_str, rest)) = split_numbered(line) {
            let _ = execute!(
                std::io::stdout(),
                SetForegroundColor(AMBER),
                Print(format!("{}  {}  ", ind, num_str)),
                ResetColor,
            );
            print_wrapped_inline(rest, indent + num_str.len() + 4, available.saturating_sub(num_str.len() + 4));
            tokio::time::sleep(Duration::from_millis(10)).await;
            continue;
        }

        if line.starts_with("> ") || line.starts_with(">") {
            let content = line.trim_start_matches('>').trim_start();
            let _ = execute!(
                std::io::stdout(),
                SetForegroundColor(AMBER_DIM),
                Print(format!("{}▌ ", ind)),
                ResetColor,
            );
            print_wrapped_inline(content, indent + 2, available.saturating_sub(2));
            tokio::time::sleep(Duration::from_millis(8)).await;
            continue;
        }

        if line.trim_start().starts_with("---") || line.trim_start().starts_with("===") {
            let _ = execute!(
                std::io::stdout(),
                SetForegroundColor(BORDER),
                Print(format!("{}{}\n", ind, "─".repeat(available))),
                ResetColor,
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
            continue;
        }

        print!("{}", ind);
        print_wrapped_inline(line, indent, available);
        let _ = io::stdout().flush();
        tokio::time::sleep(Duration::from_millis(9)).await;
    }
}

fn print_banner() {
    println!();
    println!("{}  ██████╗  █████╗ ███╗   ██╗████████╗██╗  ██╗███████╗██████╗ {}", CYAN, RESET);
    println!("{}  ██╔══██╗██╔══██╗████╗  ██║╚══██╔══╝██║  ██║██╔════╝██╔══██╗{}", CYAN, RESET);
    println!("{}  ██████╔╝███████║██╔██╗ ██║   ██║   ███████║█████╗  ██████╔╝{}", CYAN, RESET);
    println!("{}  ██╔═══╝ ██╔══██║██║╚██╗██║   ██║   ██╔══██║██╔══╝  ██╔══██╗{}", CYAN, RESET);
    println!("{}  ██║     ██║  ██║██║ ╚████║   ██║   ██║  ██║███████╗██║  ██║{}", CYAN, RESET);
    println!("{}  ╚═╝     ╚═╝  ╚═╝╚═╝  ╚═══╝   ╚═╝   ╚═╝  ╚═╝╚══════╝╚═╝  ╚═╝{}", CYAN, RESET);
    println!("{}                               █████╗ ██████╗ ███████╗██╗  ██╗{}", MAGENTA, RESET);
    println!("{}                              ██╔══██╗██╔══██╗██╔════╝╚██╗██╔╝{}", MAGENTA, RESET);
    println!("{}                              ███████║██████╔╝█████╗   ╚███╔╝ {}", MAGENTA, RESET);
    println!("{}                              ██╔══██║██╔═══╝ ██╔══╝   ██╔██╗ {}", MAGENTA, RESET);
    println!("{}                              ██║  ██║██║     ███████╗██╔╝ ██╗{}", MAGENTA, RESET);
    println!("{}                              ╚═╝  ╚═╝╚═╝     ╚══════╝╚═╝  ╚═╝{}", MAGENTA, RESET);
    println!();
    println!("{}  panther system command interface{}", S2, RESET);
    println!();

    print!("  ");
    let cmds = ["/help", "/new", "/stop", "/clear", "/exit"];
    for (i, cmd) in cmds.iter().enumerate() {
        if i > 0 { print!("{}  ·  {}", S2, RESET); }
        print!("{}{}{}", S4, cmd, RESET);
    }
    println!("\n");
}

fn print_ctrl_c_hint() {
    println!();
    println!("  {}ctrl+c  ·  /exit to quit{}", GREY, RESET);
    println!();
}

fn print_goodbye() {
    println!();
    println!("  {}{}goodbye.{}", BOLD, CYAN, RESET);
    println!();
}

fn print_tool_call(hint: &str) {
    let clean        = strip_emoji_prefix(hint);
    let (name, args) = split_tool_hint(clean);
    print!("  {}◆{} {}{}{}", CYAN, RESET, S3, name, RESET);
    if !args.is_empty() {
        print!("  {}{}  {}{}", S1, "·", S2, RESET);
        print!("{}{}{}", S2, args, RESET);
    }
    println!();
}

fn print_status(text: &str) {
    let clean = strip_emoji_prefix(text);
    println!("  {}·  {}{}", GREY, clean, RESET);
}

fn print_wrapped_inline(text: &str, indent: usize, available: usize) {
    if text.is_empty() { println!(); return; }

    let chars: Vec<char>  = text.chars().collect();
    let mut col           = 0usize;
    let mut in_bold       = false;
    let mut in_code       = false;
    let mut in_italic     = false;
    let mut pending_space = false;
    let mut i             = 0usize;

    while i < chars.len() {
        if chars[i] == ' ' || chars[i] == '\t' {
            pending_space = true;
            i += 1;
            continue;
        }

        let word_start = i;
        let mut j = i;
        while j < chars.len() && chars[j] != ' ' && chars[j] != '\t' {
            if j + 1 < chars.len() && chars[j] == '*' && chars[j+1] == '*' {
                j += 2;
            } else {
                j += 1;
            }
        }

        let vis        = visual_len(&chars[word_start..j]);
        let space_cost = if pending_space && col > 0 { 1 } else { 0 };

        if col > 0 && col + space_cost + vis > available {
            println!();
            print!("{}", " ".repeat(indent));
            col           = 0;
            pending_space = false;
        }

        if pending_space && col > 0 {
            print!(" ");
            col += 1;
        }
        pending_space = false;

        let mut k = word_start;
        while k < j {
            if k + 1 < chars.len() && chars[k] == '*' && chars[k+1] == '*' {
                in_bold = !in_bold;
                if in_bold {
                    let _ = execute!(std::io::stdout(), SetAttribute(Attribute::Bold), SetForegroundColor(SNOW));
                } else {
                    let _ = execute!(std::io::stdout(), SetAttribute(Attribute::Reset), SetForegroundColor(SNOW));
                }
                k += 2;
            } else if chars[k] == '`' {
                in_code = !in_code;
                if in_code {
                    let _ = execute!(std::io::stdout(), SetForegroundColor(CODE_COLOR));
                } else {
                    let _ = execute!(std::io::stdout(), SetForegroundColor(SNOW));
                }
                k += 1;
            } else if !in_code && !in_bold && chars[k] == '*' {
                in_italic = !in_italic;
                if in_italic {
                    let _ = execute!(std::io::stdout(), SetForegroundColor(SLATE_LIGHT));
                } else {
                    let _ = execute!(std::io::stdout(), SetForegroundColor(SNOW));
                }
                k += 1;
            } else {
                print!("{}", chars[k]);
                col += 1;
                k += 1;
            }
        }

        i = j;
    }

    let _ = execute!(std::io::stdout(), ResetColor, SetAttribute(Attribute::Reset));
    println!();
}

fn visual_len(chars: &[char]) -> usize {
    let mut len = 0usize;
    let mut k   = 0usize;
    while k < chars.len() {
        if k + 1 < chars.len() && chars[k] == '*' && chars[k+1] == '*' {
            k += 2;
        } else if chars[k] == '`' || chars[k] == '*' {
            k += 1;
        } else {
            len += 1;
            k += 1;
        }
    }
    len
}

fn split_tool_hint(hint: &str) -> (String, &str) {
    for sep in &[": ", " → ", " -> ", " — "] {
        if let Some(pos) = hint.find(sep) {
            return (hint[..pos].trim().to_string(), hint[pos + sep.len()..].trim());
        }
    }
    (hint.trim().to_string(), "")
}

fn strip_emoji_prefix(s: &str) -> &str {
    let mut iter = s.char_indices();
    if let Some((_, c)) = iter.next() {
        if c as u32 > 0x2000 {
            return iter.as_str().trim_start();
        }
    }
    s
}

fn split_numbered(line: &str) -> Option<(&str, &str)> {
    let mut end = 0usize;
    for c in line.chars() {
        if c.is_ascii_digit() { end += c.len_utf8(); } else { break; }
    }
    if end == 0 || end > 3 { return None; }
    let rest = &line[end..];
    if rest.starts_with(". ") {
        Some((&line[..end + 1], &rest[2..]))
    } else {
        None
    }
}

fn format_elapsed(d: std::time::Duration, tools: usize) -> String {
    let secs = d.as_secs_f64();
    let t    = if secs < 1.0 {
        format!("{:.0}ms", d.as_millis())
    } else {
        format!("{:.1}s", secs)
    };
    if tools > 0 {
        format!("{}  ·  {} tool call{}", t, tools, if tools == 1 { "" } else { "s" })
    } else {
        t
    }
}
