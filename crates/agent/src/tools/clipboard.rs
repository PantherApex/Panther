use std::pin::Pin;
use std::future::Future;
use serde_json::json;
use tokio::process::Command;

use super::Tool;

pub struct ClipboardTool;

async fn which(cmd: &str) -> bool {
    if cfg!(target_os = "windows") {
        Command::new("where").arg(cmd).output().await.map(|o| o.status.success()).unwrap_or(false)
    } else {
        Command::new("which").arg(cmd).output().await.map(|o| o.status.success()).unwrap_or(false)
    }
}

async fn read_clipboard() -> Result<String, String> {
    if cfg!(target_os = "macos") {
        let out = Command::new("pbpaste").output().await
            .map_err(|e| format!("pbpaste failed: {}", e))?;
        return Ok(String::from_utf8_lossy(&out.stdout).to_string());
    }

    if cfg!(target_os = "windows") {
        let script = "Get-Clipboard";
        let out = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .output().await
            .map_err(|e| format!("powershell clipboard read failed: {}", e))?;
        return Ok(String::from_utf8_lossy(&out.stdout).trim_end_matches('\n').to_string());
    }

    if which("xclip").await {
        let out = Command::new("xclip")
            .args(["-selection", "clipboard", "-o"])
            .output().await
            .map_err(|e| format!("xclip failed: {}", e))?;
        if out.status.success() {
            return Ok(String::from_utf8_lossy(&out.stdout).to_string());
        }
    }

    if which("xsel").await {
        let out = Command::new("xsel")
            .args(["--clipboard", "--output"])
            .output().await
            .map_err(|e| format!("xsel failed: {}", e))?;
        if out.status.success() {
            return Ok(String::from_utf8_lossy(&out.stdout).to_string());
        }
    }

    if which("wl-paste").await {
        let out = Command::new("wl-paste")
            .output().await
            .map_err(|e| format!("wl-paste failed: {}", e))?;
        if out.status.success() {
            return Ok(String::from_utf8_lossy(&out.stdout).to_string());
        }
    }

    Err("No clipboard tool available. Install: xclip, xsel (X11), or wl-clipboard (Wayland)".to_string())
}

async fn write_clipboard(text: &str) -> Result<(), String> {
    if cfg!(target_os = "macos") {
        let mut child = Command::new("pbcopy")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("pbcopy failed: {}", e))?;
        if let Some(stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            let mut stdin = tokio::io::BufWriter::new(stdin);
            stdin.write_all(text.as_bytes()).await
                .map_err(|e| format!("pbcopy write failed: {}", e))?;
            stdin.flush().await.map_err(|e| format!("pbcopy flush failed: {}", e))?;
        }
        child.wait().await.map_err(|e| format!("pbcopy wait failed: {}", e))?;
        return Ok(());
    }

    if cfg!(target_os = "windows") {
        let escaped = text.replace('\'', "''");
        let script = format!("Set-Clipboard -Value '{}'", escaped);
        let out = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output().await
            .map_err(|e| format!("powershell clipboard write failed: {}", e))?;
        if out.status.success() { return Ok(()); }
        return Err(format!("powershell clipboard write failed: {}", String::from_utf8_lossy(&out.stderr)));
    }

    if which("xclip").await {
        let mut child = Command::new("xclip")
            .args(["-selection", "clipboard"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("xclip write failed: {}", e))?;
        if let Some(stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            let mut stdin = tokio::io::BufWriter::new(stdin);
            stdin.write_all(text.as_bytes()).await
                .map_err(|e| format!("xclip stdin write: {}", e))?;
        }
        child.wait().await.map_err(|e| format!("xclip wait: {}", e))?;
        return Ok(());
    }

    if which("xsel").await {
        let mut child = Command::new("xsel")
            .args(["--clipboard", "--input"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("xsel write failed: {}", e))?;
        if let Some(stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            let mut stdin = tokio::io::BufWriter::new(stdin);
            stdin.write_all(text.as_bytes()).await
                .map_err(|e| format!("xsel stdin write: {}", e))?;
        }
        child.wait().await.map_err(|e| format!("xsel wait: {}", e))?;
        return Ok(());
    }

    if which("wl-copy").await {
        let mut child = Command::new("wl-copy")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("wl-copy write failed: {}", e))?;
        if let Some(stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            let mut stdin = tokio::io::BufWriter::new(stdin);
            stdin.write_all(text.as_bytes()).await
                .map_err(|e| format!("wl-copy stdin write: {}", e))?;
        }
        child.wait().await.map_err(|e| format!("wl-copy wait: {}", e))?;
        return Ok(());
    }

    Err("No clipboard write tool available. Install: xclip, xsel (X11), or wl-clipboard (Wayland)".to_string())
}

impl Tool for ClipboardTool {
    fn name(&self) -> &str { "clipboard" }

    fn description(&self) -> &str {
        "Read from or write to the system clipboard. Works cross-platform (Windows/macOS/Linux). Useful for getting text the user has copied, or placing text for the user to paste."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["read", "write"],
                    "description": "Whether to read from or write to the clipboard"
                },
                "text": {
                    "type": "string",
                    "description": "Text to write to clipboard (only required when action is 'write')"
                }
            },
            "required": ["action"]
        })
    }

    fn execute<'a>(&'a self, args: serde_json::Value) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
        Box::pin(async move {
            let action = match args.get("action").and_then(|v| v.as_str()) {
                Some(a) => a.to_string(),
                None => return "Error: missing 'action' argument".to_string(),
            };

            match action.as_str() {
                "read" => match read_clipboard().await {
                    Ok(content) => {
                        if content.is_empty() {
                            "Clipboard is empty".to_string()
                        } else {
                            format!("Clipboard contents:\n{}", content)
                        }
                    }
                    Err(e) => format!("Clipboard read failed: {}", e),
                },
                "write" => {
                    let text = match args.get("text").and_then(|v| v.as_str()) {
                        Some(t) => t.to_string(),
                        None => return "Error: missing 'text' argument for write action".to_string(),
                    };
                    match write_clipboard(&text).await {
                        Ok(()) => format!("Copied {} characters to clipboard", text.len()),
                        Err(e) => format!("Clipboard write failed: {}", e),
                    }
                }
                other => format!("Unknown action: '{}'. Use: read or write", other),
            }
        })
    }
}
