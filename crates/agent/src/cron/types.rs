use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CronSchedule {
    At { at_ms: i64 },
    Every { every_ms: u64 },
    Cron { expr: String, tz: Option<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronPayload {
    pub message: String,
    pub channel: String,
    pub chat_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CronJobState {
    pub next_run_at_ms: Option<i64>,
    pub last_run_at_ms: Option<i64>,
    pub last_status: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub schedule: CronSchedule,
    pub payload: CronPayload,
    pub state: CronJobState,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub delete_after_run: bool,
}

pub fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

pub fn compute_next_run(schedule: &CronSchedule, now_ms: i64) -> Option<i64> {
    match schedule {
        CronSchedule::At { at_ms } => {
            if *at_ms > now_ms { Some(*at_ms) } else { None }
        }
        CronSchedule::Every { every_ms } => {
            Some(now_ms + *every_ms as i64)
        }
        CronSchedule::Cron { expr, tz } => {
            use cron::Schedule;
            use std::str::FromStr;
            let schedule = Schedule::from_str(expr).ok()?;
            let base: DateTime<Utc> = DateTime::from_timestamp_millis(now_ms)?;
            if let Some(tz_str) = tz {
                let tz: chrono_tz::Tz = tz_str.parse().ok()?;
                let base_local = base.with_timezone(&tz);
                schedule.after(&base_local).next().map(|dt| dt.timestamp_millis())
            } else {
                schedule.after(&base).next().map(|dt| dt.timestamp_millis())
            }
        }
    }
}
