mod config;
mod setup;

use std::io::{self, Write};
use std::thread;
use std::time::Duration;

// ── ANSI helpers ────────────────────────────────────────────────────────────

const RESET:    &str = "\x1b[0m";
const BOLD:     &str = "\x1b[1m";
const DIM:      &str = "\x1b[2m";

// Silver / chrome palette (256-colour)
const S0:  &str = "\x1b[38;5;236m"; // darkest  #303030
const S1:  &str = "\x1b[38;5;240m"; // #585858
const S2:  &str = "\x1b[38;5;244m"; // #808080
const S3:  &str = "\x1b[38;5;248m"; // #a8a8a8
const S4:  &str = "\x1b[38;5;252m"; // #d0d0d0
const S5:  &str = "\x1b[38;5;255m"; // near-white highlight
const CYAN:     &str = "\x1b[96m";
const MAGENTA:  &str = "\x1b[95m";
const GREEN:    &str = "\x1b[92m";
const TEAL:     &str = "\x1b[36m";
const MAUVE:    &str = "\x1b[35m";
const RED:      &str = "\x1b[91m";
const GREY:     &str = "\x1b[90m";

fn hide_cursor() { print!("\x1b[?25l"); let _ = io::stdout().flush(); }
fn show_cursor() { print!("\x1b[?25h"); let _ = io::stdout().flush(); }
fn clear_line()  { print!("\r\x1b[2K"); let _ = io::stdout().flush(); }

// ── Silver shimmer on a single line ─────────────────────────────────────────
//
// Splits `text` into chars and paints a travelling "bright window"
// that sweeps left-to-right, giving the ChatGPT-style silver glint.

fn shimmer_line(text: &str, passes: u8, delay_ms: u64) {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let window = 6usize; // half-width of the bright zone

    hide_cursor();

    for _pass in 0..passes {
        // sweep forward
        for centre in 0..=(len + window) {
            clear_line();
            print!("  ");
            for (i, ch) in chars.iter().enumerate() {
                let dist = if centre > i { centre - i } else { i - centre };
                let colour = if dist == 0 {
                    S5
                } else if dist <= 1 {
                    S4
                } else if dist <= 2 {
                    S3
                } else if dist <= 3 {
                    S2
                } else if dist <= 5 {
                    S1
                } else {
                    S0
                };
                print!("{}{}{}", colour, ch, RESET);
            }
            let _ = io::stdout().flush();
            thread::sleep(Duration::from_millis(delay_ms));
        }
    }

    // settle to solid dim-silver
    clear_line();
    print!("  {}{}{}", S3, text, RESET);
    println!();
    show_cursor();
}

// ── "Thinking…" silver typewriter + shimmer ──────────────────────────────────

fn thinking_line(label: &str) {
    let dots = ["   ", ".  ", ".. ", "..."];
    hide_cursor();
    for cycle in 0..12 {
        clear_line();
        let dot = dots[cycle % 4];

        // Build a shimmer effect over the whole string inline
        let full = format!("{}{}", label, dot);
        let chars: Vec<char> = full.chars().collect();
        let len = chars.len();
        let centre = (cycle * 2) % (len + 4);

        print!("  ");
        for (i, ch) in chars.iter().enumerate() {
            let dist = if centre > i { centre - i } else { i - centre };
            let colour = if dist == 0 {
                S5
            } else if dist <= 2 {
                S4
            } else if dist <= 4 {
                S3
            } else {
                S1
            };
            print!("{}{}{}", colour, ch, RESET);
        }
        let _ = io::stdout().flush();
        thread::sleep(Duration::from_millis(90));
    }
    clear_line();
    show_cursor();
}

// ── Ruled divider ────────────────────────────────────────────────────────────

fn rule(colour: &str) {
    println!("{}  {}{}", colour, "─".repeat(69), RESET);
}

fn thin_rule() {
    println!("{}  {}{}", S1, "·".repeat(69), RESET);
}

// ── Logo with per-row shimmer ────────────────────────────────────────────────

fn print_logo() {
    println!();
    thinking_line("Initialising Panther Apex");

    // PANTHER — swept with shimmer on each row
    let panther_rows = [
        "  ██████╗  █████╗ ███╗   ██╗████████╗██╗  ██╗███████╗██████╗ ",
        "  ██╔══██╗██╔══██╗████╗  ██║╚══██╔══╝██║  ██║██╔════╝██╔══██╗",
        "  ██████╔╝███████║██╔██╗ ██║   ██║   ███████║█████╗  ██████╔╝",
        "  ██╔═══╝ ██╔══██║██║╚██╗██║   ██║   ██╔══██║██╔══╝  ██╔══██╗",
        "  ██║     ██║  ██║██║ ╚████║   ██║   ██║  ██║███████╗██║  ██║",
        "  ╚═╝     ╚═╝  ╚═╝╚═╝  ╚═══╝   ╚═╝   ╚═╝  ╚═╝╚══════╝╚═╝  ╚═╝",
    ];

    for row in &panther_rows {
        shimmer_line(row, 1, 14);
    }

    // APEX — a single shimmer pass
    let apex_rows = [
        "                               █████╗ ██████╗ ███████╗██╗  ██╗",
        "                              ██╔══██╗██╔══██╗██╔════╝╚██╗██╔╝",
        "                              ███████║██████╔╝█████╗   ╚███╔╝ ",
        "                              ██╔══██║██╔═══╝ ██╔══╝   ██╔██╗ ",
        "                              ██║  ██║██║     ███████╗██╔╝ ██╗",
        "                              ╚═╝  ╚═╝╚═╝     ╚══════╝╚═╝  ╚═╝",
    ];

    for row in &apex_rows {
        // APEX rows in magenta-tinted silver
        let chars: Vec<char> = row.chars().collect();
        print!("  ");
        for ch in &chars {
            print!("{}{}{}", MAGENTA, ch, RESET);
        }
        println!();
        thread::sleep(Duration::from_millis(30));
    }

    println!();
}

// ── Subtitle + tagline ────────────────────────────────────────────────────────

fn print_header() {
    rule(S2);
    println!();
    shimmer_line("  Panther System Setup & Configuration", 1, 18);
    println!("  {}You can re-run this at any time:{} {}panther-install{}", GREY, RESET, CYAN, RESET);
    println!();
    rule(S2);
    println!();
}

// ── Post-save summary panel ───────────────────────────────────────────────────

fn print_success_panel(path: &std::path::Path) {
    println!();
    rule(S3);
    println!();
    shimmer_line("  ✦  Panther Apex  —  ready", 2, 20);
    println!();
    thin_rule();
    println!();
    println!("  {}Config saved to:{}", GREY, RESET);
    println!("  {}  {}{}", TEAL, path.display(), RESET);
    println!();
    thin_rule();
    println!();
    println!("  {}{}Commands{}", BOLD, S4, RESET);
    println!("  {}panther{}            — start the agent", TEAL, RESET);
    println!("  {}panther-install{}    — reconfigure settings", TEAL, RESET);
    println!();
    thin_rule();
    println!();
    println!("  {}{}Storage{}  {}~/.panther/{}", BOLD, S4, RESET, GREY, RESET);
    println!("  {}workspace/{}         — agent files, skills, memory", MAUVE, RESET);
    println!("  {}sessions/{}          — conversation history", MAUVE, RESET);
    println!("  {}config.toml{}        — system configuration", MAUVE, RESET);
    println!();
    rule(S3);
    println!();
}

// ── Animated confirm prompt ───────────────────────────────────────────────────

fn animated_prompt(question: &str) -> bool {
    // Brief shimmer before the prompt appears
    shimmer_line(&format!("  {}", question), 1, 22);
    dialoguer::Confirm::new()
        .with_prompt(format!("  {}", question))
        .default(true)
        .interact()
        .unwrap_or(false)
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    // Register a SIGINT/SIGTERM handler so we always restore the cursor
    // (best-effort; full signal handling needs the `ctrlc` crate)
    print_logo();
    print_header();

    thinking_line("Loading configuration");

    let cfg = config::PantherConfig::load()
        .await
        .unwrap_or_else(|_| config::PantherConfig::default_config());

    let cfg = setup::run(cfg);

    thinking_line("Saving configuration");

    match cfg.save().await {
        Ok(path) => {
            print_success_panel(&path);
        }
        Err(e) => {
            println!();
            println!("  {}✗  Could not save config:{} {}", RED, RESET, e);
            println!("  {}Tip:{} manually create {}~/.panther/config.toml{}", GREY, RESET, TEAL, RESET);
            println!();
            std::process::exit(1);
        }
    }

    // ── Register on login ──────────────────────────────────────────────────
    let register = animated_prompt("Register Panther to start automatically on login?");

    if register {
        thinking_line("Registering startup entry");
        if let Some(binary_path) = panther_binary_path() {
            setup::register_startup(&binary_path);
            println!("  {}✓  Registered.{}", GREEN, RESET);
        } else {
            println!("  {}⚠  Could not locate panther binary — run 'panther' manually.{}", GREY, RESET);
        }
        println!();
    }

    // ── Start now ──────────────────────────────────────────────────────────
    let start_now = animated_prompt("Start Panther now?");

    if start_now {
        println!();
        shimmer_line("  ✦  Launching Panther Apex…", 3, 16);
        println!();
        println!("  {}Your agent is live on Telegram. Send it a message.{}", GREY, RESET);
        println!("  {}Press Ctrl+C to stop.{}", GREY, RESET);
        println!();

        match panther_binary_path() {
            Some(path) => {
                let status = std::process::Command::new(&path).status();
                if let Err(e) = status {
                    println!("  {}Could not start Panther:{} {}", RED, RESET, e);
                    println!("  Run {}panther{} manually.", TEAL, RESET);
                }
            }
            None => {
                println!("  {}Could not locate panther binary.{} Run {}panther{} manually.", RED, RESET, TEAL, RESET);
            }
        }
    } else {
        println!();
        println!("  {}Run {}panther{}{} whenever you're ready.{}", GREY, TEAL, GREY, DIM, RESET);
        println!();
    }
}

fn panther_binary_path() -> Option<std::path::PathBuf> {
    std::env::current_exe().ok().and_then(|exe| {
        exe.parent().map(|dir| {
            #[cfg(windows)]
            let name = "panther.exe";
            #[cfg(not(windows))]
            let name = "panther";
            dir.join(name)
        })
    })
}