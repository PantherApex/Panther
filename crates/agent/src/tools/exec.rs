use std::pin::Pin;
use std::future::Future;
use std::time::Duration;
use serde_json::json;

use super::Tool;

pub struct ExecTool {
    pub working_dir: String,
    pub default_timeout_secs: u64,
    pub path_append: String,
}

impl ExecTool {
    pub fn new(working_dir: String, default_timeout_secs: u64, path_append: String) -> Self {
        Self { working_dir, default_timeout_secs, path_append }
    }
}

fn is_dangerous(command: &str) -> Option<&'static str> {
    let patterns: &[(&str, &'static str)] = &[
        ("rm -rf", "destructive rm -rf"),
        ("rm -fr", "destructive rm -fr"),
        ("rm -f /", "destructive rm of root"),
        ("del /f", "destructive del /f"),
        ("rmdir /s", "destructive rmdir /s"),
        ("mkfs", "disk format"),
        ("diskpart", "disk partition tool"),
        ("dd if=", "raw disk write"),
        (">/dev/sd", "raw disk overwrite"),
        (":(){ :", "fork bomb"),
        ("format c:", "format C drive"),
        ("format /", "disk format"),
    ];
    for (pat, reason) in patterns {
        if command.to_lowercase().contains(pat) {
            return Some(reason);
        }
    }
    None
}

fn build_path_env(path_append: &str) -> Option<String> {
    if path_append.is_empty() {
        return None;
    }
    let current = std::env::var("PATH").unwrap_or_default();
    if cfg!(target_os = "windows") {
        Some(format!("{};{}", current, path_append))
    } else {
        Some(format!("{}:{}", current, path_append))
    }
}

fn shell_command(command: &str, path_env: Option<&str>) -> tokio::process::Command {
    if cfg!(target_os = "windows") {
        let mut cmd = tokio::process::Command::new("powershell");
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", command]);
        if let Some(p) = path_env {
            cmd.env("PATH", p);
        }
        cmd
    } else {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(command);
        if let Some(p) = path_env {
            cmd.env("PATH", p);
        }
        cmd
    }
}

impl Tool for ExecTool {
    fn name(&self) -> &str { "exec" }

    fn description(&self) -> &str {
        "Execute a shell command. Returns stdout, stderr, and exit code. Use platform-appropriate commands (see runtime context for OS and shell type)."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute. Write commands appropriate for the OS shown in runtime context."
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Timeout in seconds (optional, uses configured default if omitted)"
                }
            },
            "required": ["command"]
        })
    }

    fn execute<'a>(&'a self, args: serde_json::Value) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
        let working_dir = self.working_dir.clone();
        let default_timeout = self.default_timeout_secs;
        let path_env = build_path_env(&self.path_append);
        Box::pin(async move {
            let command = match args.get("command").and_then(|v| v.as_str()) {
                Some(c) => c.to_string(),
                None => return "Error: missing 'command' argument".to_string(),
            };

            if let Some(reason) = is_dangerous(&command) {
                return format!("Error: command blocked — {}. If you need to perform this operation, break it into safer steps.", reason);
            }

            let timeout_secs = args.get("timeout_secs")
                .and_then(|v| v.as_u64())
                .unwrap_or(default_timeout);

            let mut cmd = shell_command(&command, path_env.as_deref());
            cmd.current_dir(&working_dir);

            let result = tokio::time::timeout(
                Duration::from_secs(timeout_secs),
                cmd.output()
            ).await;

            match result {
                Ok(Ok(output)) => {
                    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                    let code = output.status.code().unwrap_or(-1);
                    if stderr.is_empty() {
                        format!("Exit: {}\n{}", code, stdout)
                    } else {
                        format!("Exit: {}\nStdout:\n{}\nStderr:\n{}", code, stdout, stderr)
                    }
                }
                Ok(Err(e)) => format!("Error executing command: {}", e),
                Err(_) => format!("Command timed out after {} seconds", timeout_secs),
            }
        })
    }
}
