use std::pin::Pin;
use std::future::Future;
use std::sync::{Arc, RwLock};
use serde_json::json;

use crate::cron::{CronService, CronSchedule};
use super::Tool;

#[derive(Clone)]
struct CronContext {
    channel: String,
    chat_id: String,
}

pub struct CronTool {
    service: CronService,
    context: Arc<RwLock<CronContext>>,
}

impl CronTool {
    pub fn new(service: CronService) -> Self {
        Self {
            service,
            context: Arc::new(RwLock::new(CronContext {
                channel: String::new(),
                chat_id: String::new(),
            })),
        }
    }

    pub fn set_context(&self, channel: String, chat_id: String) {
        if let Ok(mut ctx) = self.context.write() {
            ctx.channel = channel;
            ctx.chat_id = chat_id;
        }
    }
}

impl Tool for CronTool {
    fn name(&self) -> &str { "cron" }

    fn description(&self) -> &str {
        "Schedule reminders and recurring tasks. Actions: add, list, remove."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["add", "list", "remove"],
                    "description": "Action to perform"
                },
                "message": {
                    "type": "string",
                    "description": "Reminder message (required for add)"
                },
                "every_seconds": {
                    "type": "integer",
                    "description": "Repeat every N seconds (for add)"
                },
                "cron_expr": {
                    "type": "string",
                    "description": "7-field cron expression: sec min hour day month weekday year. E.g. '0 0 9 * * * *' = 9am daily"
                },
                "tz": {
                    "type": "string",
                    "description": "IANA timezone for cron_expr e.g. 'America/New_York'"
                },
                "at": {
                    "type": "string",
                    "description": "ISO datetime for one-time run e.g. '2026-06-01T09:00:00'"
                },
                "job_id": {
                    "type": "string",
                    "description": "Job ID (required for remove)"
                }
            },
            "required": ["action"]
        })
    }

    fn execute<'a>(&'a self, args: serde_json::Value) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
        let service = self.service.clone();
        let context = Arc::clone(&self.context);
        Box::pin(async move {
            let action = match args.get("action").and_then(|v| v.as_str()) {
                Some(a) => a.to_string(),
                None => return "Error: missing 'action'".to_string(),
            };

            match action.as_str() {
                "list" => {
                    let jobs = service.list_jobs().await;
                    if jobs.is_empty() {
                        return "No scheduled jobs.".to_string();
                    }
                    let lines: Vec<String> = jobs.iter().map(|j| {
                        let next = j.state.next_run_at_ms
                            .and_then(|ms| chrono::DateTime::from_timestamp_millis(ms))
                            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                            .unwrap_or_else(|| "unknown".to_string());
                        format!("- {} (id: {}, next: {})", j.name, j.id, next)
                    }).collect();
                    format!("Scheduled jobs:\n{}", lines.join("\n"))
                }

                "remove" => {
                    let job_id = match args.get("job_id").and_then(|v| v.as_str()) {
                        Some(id) => id.to_string(),
                        None => return "Error: job_id required for remove".to_string(),
                    };
                    if service.remove_job(&job_id).await {
                        format!("Removed job {}", job_id)
                    } else {
                        format!("Job {} not found", job_id)
                    }
                }

                "add" => {
                    let message = match args.get("message").and_then(|v| v.as_str()) {
                        Some(m) => m.to_string(),
                        None => return "Error: message required for add".to_string(),
                    };

                    let ctx = context.read().map(|c| c.clone()).unwrap_or(CronContext {
                        channel: String::new(),
                        chat_id: String::new(),
                    });

                    if ctx.chat_id.is_empty() {
                        return "Error: no session context available".to_string();
                    }

                    let (schedule, delete_after) = if let Some(secs) = args.get("every_seconds").and_then(|v| v.as_u64()) {
                        (CronSchedule::Every { every_ms: secs * 1000 }, false)
                    } else if let Some(expr) = args.get("cron_expr").and_then(|v| v.as_str()) {
                        let tz = args.get("tz").and_then(|v| v.as_str()).map(|s| s.to_string());
                        if let Some(ref tz_str) = tz {
                            if tz_str.parse::<chrono_tz::Tz>().is_err() {
                                return format!("Error: unknown timezone '{}'", tz_str);
                            }
                        }
                        (CronSchedule::Cron { expr: expr.to_string(), tz }, false)
                    } else if let Some(at_str) = args.get("at").and_then(|v| v.as_str()) {
                        match chrono::NaiveDateTime::parse_from_str(at_str, "%Y-%m-%dT%H:%M:%S") {
                            Ok(dt) => {
                                let at_ms = dt.and_utc().timestamp_millis();
                                (CronSchedule::At { at_ms }, true)
                            }
                            Err(_) => return format!("Error: invalid datetime '{}', use ISO format e.g. 2026-06-01T09:00:00", at_str),
                        }
                    } else {
                        return "Error: one of every_seconds, cron_expr, or at is required".to_string();
                    };

                    let name = message.chars().take(40).collect::<String>();
                    let job = service.add_job(name, schedule, message, ctx.channel, ctx.chat_id, delete_after).await;
                    format!("Created job '{}' (id: {})", job.name, job.id)
                }

                _ => format!("Unknown action: {}", action),
            }
        })
    }
}
