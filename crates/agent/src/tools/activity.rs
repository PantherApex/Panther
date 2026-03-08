use std::pin::Pin;
use std::future::Future;
use std::sync::Arc;
use chrono::{NaiveDate, Utc};
use serde_json::json;
use tokio::process::Command;

use activity_tracker::ActivityAnalyzer;
use super::Tool;

pub struct ActivityTool {
    analyzer: Arc<ActivityAnalyzer>,
}

impl ActivityTool {
    pub fn new(analyzer: Arc<ActivityAnalyzer>) -> Self {
        Self { analyzer }
    }
}

impl Tool for ActivityTool {
    fn name(&self) -> &str {
        "query_activity"
    }

    fn description(&self) -> &str {
        "Query what the user was working on during a time period, or get the current live activity state. Can also close a process or window by PID or name when the user confirms they want it closed."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["current", "last_hour", "last_2h", "last_4h", "last_8h", "today", "date_range", "close_process"],
                    "description": "Operation: 'current' for live state, time range queries for history, 'close_process' to terminate a running process/window"
                },
                "hours_back": {
                    "type": "number",
                    "description": "For custom lookback: number of hours to look back (used when operation is 'last_hour', 'last_2h', etc. or provide a custom value)"
                },
                "date": {
                    "type": "string",
                    "description": "For 'today' or specific date in YYYY-MM-DD format"
                },
                "start_time": {
                    "type": "string",
                    "description": "For date_range: ISO 8601 start datetime"
                },
                "end_time": {
                    "type": "string",
                    "description": "For date_range: ISO 8601 end datetime"
                },
                "pid": {
                    "type": "integer",
                    "description": "For close_process: the process ID to terminate"
                },
                "process_name": {
                    "type": "string",
                    "description": "For close_process: process name to terminate (used if pid is not provided)"
                }
            },
            "required": ["operation"]
        })
    }

    fn execute<'a>(&'a self, args: serde_json::Value) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
        Box::pin(async move {
            let operation = match args.get("operation").and_then(|v| v.as_str()) {
                Some(op) => op.to_string(),
                None => return "Missing required parameter: operation".to_string(),
            };

            match operation.as_str() {
                "current" => self.analyzer.get_current_state().await,

                "last_hour" => self.analyzer.last_n_hours(1).await,
                "last_2h" => self.analyzer.last_n_hours(2).await,
                "last_4h" => self.analyzer.last_n_hours(4).await,
                "last_8h" => self.analyzer.last_n_hours(8).await,

                "today" => {
                    if let Some(date_str) = args.get("date").and_then(|v| v.as_str()) {
                        match NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
                            Ok(date) => self.analyzer.daily_summary(date).await,
                            Err(_) => self.analyzer.today_summary().await,
                        }
                    } else {
                        self.analyzer.today_summary().await
                    }
                }

                "date_range" => {
                    let start = args
                        .get("start_time")
                        .and_then(|v| v.as_str())
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .map(|dt| dt.with_timezone(&Utc));

                    let end = args
                        .get("end_time")
                        .and_then(|v| v.as_str())
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .map(|dt| dt.with_timezone(&Utc));

                    match (start, end) {
                        (Some(s), Some(e)) => self.analyzer.query_time_range(s, e).await,
                        _ => {
                            let hours = args
                                .get("hours_back")
                                .and_then(|v| v.as_f64())
                                .unwrap_or(1.0) as u64;
                            self.analyzer.last_n_hours(hours.max(1)).await
                        }
                    }
                }

                "close_process" => {
                    let pid = args.get("pid").and_then(|v| v.as_u64()).map(|p| p as u32);
                    let name = args.get("process_name").and_then(|v| v.as_str()).map(|s| s.to_string());
                    terminate_process(pid, name.as_deref()).await
                }

                _ => {
                    let hours = args
                        .get("hours_back")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(1.0)
                        .max(0.1) as u64;
                    self.analyzer.last_n_hours(hours.max(1)).await
                }
            }
        })
    }
}

async fn terminate_process(pid: Option<u32>, name: Option<&str>) -> String {
    match (pid, name) {
        (Some(p), _) => terminate_by_pid(p).await,
        (None, Some(n)) => terminate_by_name(n).await,
        (None, None) => "No process identifier provided. Specify either 'pid' or 'process_name'.".to_string(),
    }
}

async fn terminate_by_pid(pid: u32) -> String {
    if cfg!(target_os = "windows") {
        let script = format!("Stop-Process -Id {} -Force -ErrorAction Stop; 'Process {} terminated.'", pid, pid);
        run_ps(&script).await
    } else {
        match Command::new("kill").args(["-15", &pid.to_string()]).output().await {
            Ok(o) if o.status.success() => format!("Process {} terminated (SIGTERM).", pid),
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr).to_string();
                format!("Failed to terminate process {}: {}", pid, stderr.trim())
            }
            Err(e) => format!("Failed to execute kill: {}", e),
        }
    }
}

async fn terminate_by_name(name: &str) -> String {
    if cfg!(target_os = "windows") {
        let script = format!(
            "Stop-Process -Name '{}' -Force -ErrorAction Stop; 'Process {} terminated.'",
            name.replace('\'', "''"),
            name
        );
        run_ps(&script).await
    } else if cfg!(target_os = "macos") {
        let script = format!("pkill -15 '{}' && echo 'Sent SIGTERM to {}' || echo 'No process found matching {}'", name, name, name);
        match Command::new("sh").args(["-c", &script]).output().await {
            Ok(o) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            Err(e) => format!("Failed to terminate {}: {}", name, e),
        }
    } else {
        let script = format!("pkill -15 '{}' && echo 'Sent SIGTERM to {}' || echo 'No process found matching {}'", name, name, name);
        match Command::new("sh").args(["-c", &script]).output().await {
            Ok(o) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            Err(e) => format!("Failed to terminate {}: {}", name, e),
        }
    }
}

async fn run_ps(script: &str) -> String {
    match Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .await
    {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr).to_string();
            format!("Error: {}", stderr.trim())
        }
        Err(e) => format!("PowerShell execution failed: {}", e),
    }
}
