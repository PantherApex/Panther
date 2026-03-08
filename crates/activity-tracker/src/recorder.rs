use std::collections::HashMap;
use std::path::PathBuf;
use chrono::{DateTime, Local, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use shared::errors::PantherResult;
use crate::monitor::{ActivitySnapshot, ProcessInfo};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertCondition {
    pub kind: AlertKind,
    pub description: String,
    pub process_name: Option<String>,
    pub pid: Option<u32>,
    pub document: Option<String>,
    pub app_name: Option<String>,
    pub duration_mins: u64,
    pub cpu_percent: Option<f32>,
    pub memory_mb: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AlertKind {
    LongRunningBackgroundProcess,
    DocumentOpenUnused,
}

#[derive(Debug, Clone)]
struct ActiveSession {
    started_at: DateTime<Utc>,
    app_name: String,
    document: Option<String>,
    window_title: String,
}

pub struct ActivityRecorder {
    activity_dir: PathBuf,
    current_session: Option<ActiveSession>,
    window_first_seen: HashMap<String, DateTime<Utc>>,
    window_last_active: HashMap<String, DateTime<Utc>>,
    alerted_pids: HashMap<u32, DateTime<Utc>>,
    alerted_windows: HashMap<String, DateTime<Utc>>,
    alert_threshold_mins: u64,
}

impl ActivityRecorder {
    pub fn new(activity_dir: PathBuf, alert_threshold_mins: u64) -> Self {
        Self {
            activity_dir,
            current_session: None,
            window_first_seen: HashMap::new(),
            window_last_active: HashMap::new(),
            alerted_pids: HashMap::new(),
            alerted_windows: HashMap::new(),
            alert_threshold_mins,
        }
    }

    pub async fn process_snapshot(&mut self, snapshot: &ActivitySnapshot) -> Vec<AlertCondition> {
        self.update_window_tracking(snapshot);
        let session_ended = self.advance_session(snapshot).await;
        if let Some((completed, ended_at)) = session_ended {
            let _ = self.append_session_to_log(&completed, ended_at).await;
        }
        let _ = self.write_live_state(snapshot).await;
        self.detect_alerts(snapshot)
    }

    fn update_window_tracking(&mut self, snapshot: &ActivitySnapshot) {
        let now = snapshot.captured_at;
        let active_title = snapshot.active_window.as_ref().map(|w| w.title.clone());

        for win in &snapshot.visible_windows {
            self.window_first_seen.entry(win.title.clone()).or_insert(now);
        }

        if let Some(ref title) = active_title {
            self.window_last_active.insert(title.clone(), now);
            self.window_first_seen.entry(title.clone()).or_insert(now);
        }

        let visible_set: std::collections::HashSet<String> = snapshot
            .visible_windows
            .iter()
            .map(|w| w.title.clone())
            .chain(active_title.into_iter())
            .collect();

        self.window_first_seen.retain(|k, _| visible_set.contains(k));
        self.window_last_active.retain(|k, _| visible_set.contains(k));
    }

    async fn advance_session(&mut self, snapshot: &ActivitySnapshot) -> Option<(ActiveSession, DateTime<Utc>)> {
        let new_active = snapshot.active_window.as_ref()?;
        let now = snapshot.captured_at;

        match &self.current_session {
            None => {
                self.current_session = Some(ActiveSession {
                    started_at: now,
                    app_name: new_active.app_name.clone(),
                    document: new_active.document_hint.clone(),
                    window_title: new_active.title.clone(),
                });
                None
            }
            Some(existing) if existing.window_title == new_active.title => None,
            Some(existing) => {
                let completed = existing.clone();
                let ended_at = now;
                self.current_session = Some(ActiveSession {
                    started_at: now,
                    app_name: new_active.app_name.clone(),
                    document: new_active.document_hint.clone(),
                    window_title: new_active.title.clone(),
                });
                Some((completed, ended_at))
            }
        }
    }

    async fn append_session_to_log(&self, session: &ActiveSession, ended_at: DateTime<Utc>) -> PantherResult<()> {
        let local_start = session.started_at.with_timezone(&Local);
        let date = local_start.date_naive();
        let path = self.daily_log_path(date);

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let duration_secs = (ended_at - session.started_at).num_seconds().max(0) as u64;
        if duration_secs < 5 {
            return Ok(());
        }

        let local_end = ended_at.with_timezone(&Local);
        let duration_str = format_duration(duration_secs);
        let doc_col = session.document.as_deref().unwrap_or("—");

        let mut entry = String::new();

        if !path.exists() {
            entry.push_str(&format!(
                "---\ndate: {}\ngenerated_by: panther-activity-tracker\n---\n\n# Activity Log — {}\n\n## Sessions\n\n| Start | End | Duration | Application | Document |\n|-------|-----|----------|-------------|----------|\n",
                date,
                local_start.format("%A, %d %B %Y")
            ));
        }

        entry.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            local_start.format("%H:%M:%S"),
            local_end.format("%H:%M:%S"),
            duration_str,
            escape_md_cell(&session.app_name),
            escape_md_cell(doc_col),
        ));

        entry.push_str(&format!(
            "\n### [{}] {} · {}\n**Duration:** {}  \n**Window Title:** {}  \n\n---\n",
            local_start.format("%H:%M:%S"),
            session.app_name,
            doc_col,
            duration_str,
            session.window_title,
        ));

        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;
        file.write_all(entry.as_bytes()).await?;
        Ok(())
    }

    pub async fn write_live_state(&self, snapshot: &ActivitySnapshot) -> PantherResult<()> {
        let path = self.activity_dir.join("LIVE.md");
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let now_local = snapshot.captured_at.with_timezone(&Local);
        let mut buf = format!(
            "---\nupdated: {}\n---\n\n# Live Activity\n\n",
            snapshot.captured_at.to_rfc3339()
        );

        if let Some(ref active) = snapshot.active_window {
            let session_start = self.current_session.as_ref().map(|s| s.started_at);
            let duration_str = session_start
                .map(|s| {
                    let secs = (snapshot.captured_at - s).num_seconds().max(0) as u64;
                    format_duration(secs)
                })
                .unwrap_or_else(|| "—".to_string());

            buf.push_str(&format!(
                "**Active Application:** {}  \n",
                active.app_name
            ));
            if let Some(ref doc) = active.document_hint {
                buf.push_str(&format!("**Active Document:** {}  \n", doc));
            }
            buf.push_str(&format!(
                "**Current Session Duration:** {}  \n**Captured At:** {}  \n\n",
                duration_str,
                now_local.format("%H:%M:%S")
            ));
        } else {
            buf.push_str("**Status:** No active window detected  \n\n");
        }

        if !snapshot.visible_windows.is_empty() {
            buf.push_str("## All Open Windows\n\n| Application | Document / Title |\n|-------------|------------------|\n");
            for w in &snapshot.visible_windows {
                let doc = w.document_hint.as_deref().unwrap_or(&w.title);
                buf.push_str(&format!(
                    "| {} | {} |\n",
                    escape_md_cell(&w.app_name),
                    escape_md_cell(doc)
                ));
            }
            buf.push('\n');
        }

        let notable_procs: Vec<&ProcessInfo> = snapshot
            .top_processes
            .iter()
            .filter(|p| p.cpu_percent > 0.1 || p.memory_mb > 100)
            .take(20)
            .collect();

        if !notable_procs.is_empty() {
            buf.push_str("## Background Processes (Top by CPU)\n\n| PID | Process | CPU% | Memory | Running For |\n|-----|---------|------|--------|-------------|\n");
            for p in notable_procs {
                buf.push_str(&format!(
                    "| {} | {} | {:.1}% | {} MB | {} |\n",
                    p.pid,
                    escape_md_cell(&p.name),
                    p.cpu_percent,
                    p.memory_mb,
                    format_duration(p.elapsed_secs),
                ));
            }
            buf.push('\n');
        }

        let tmp = path.with_extension("md.tmp");
        tokio::fs::write(&tmp, buf.as_bytes()).await?;
        tokio::fs::rename(&tmp, &path).await?;
        Ok(())
    }

    fn detect_alerts(&mut self, snapshot: &ActivitySnapshot) -> Vec<AlertCondition> {
        let mut alerts = Vec::new();
        let now = snapshot.captured_at;
        let threshold = chrono::Duration::minutes(self.alert_threshold_mins as i64);
        let re_alert_gap = chrono::Duration::hours(1);


        let active_pids: std::collections::HashSet<u32> = snapshot
            .visible_windows
            .iter()
            .chain(snapshot.active_window.iter())
            .filter_map(|w| w.pid)
            .collect();

        for proc in &snapshot.top_processes {
            if proc.elapsed_secs < (self.alert_threshold_mins * 60) {
                continue;
            }
            if active_pids.contains(&proc.pid) {
                continue;
            }
            if proc.cpu_percent < 0.5 && proc.memory_mb < 500 {
                continue;
            }
            if is_system_process(&proc.name) {
                continue;
            }

            let last_alerted = self.alerted_pids.get(&proc.pid).copied();
            if last_alerted.map(|t| now - t < re_alert_gap).unwrap_or(false) {
                continue;
            }

            self.alerted_pids.insert(proc.pid, now);
            alerts.push(AlertCondition {
                kind: AlertKind::LongRunningBackgroundProcess,
                description: format!(
                    "Process '{}' (PID {}) has been running for {} with no open window, using {:.1}% CPU and {} MB memory.",
                    proc.name,
                    proc.pid,
                    format_duration(proc.elapsed_secs),
                    proc.cpu_percent,
                    proc.memory_mb,
                ),
                process_name: Some(proc.name.clone()),
                pid: Some(proc.pid),
                document: None,
                app_name: None,
                duration_mins: proc.elapsed_secs / 60,
                cpu_percent: Some(proc.cpu_percent),
                memory_mb: Some(proc.memory_mb),
            });
        }

        for (title, first_seen) in &self.window_first_seen {
            if now - *first_seen < threshold {
                continue;
            }
            if title == snapshot.active_window.as_ref().map(|w| &w.title).unwrap_or(&String::new()) {
                continue;
            }
            let last_active = self.window_last_active.get(title).copied();
            let idle_since = last_active.unwrap_or(*first_seen);
            if now - idle_since < threshold {
                continue;
            }

            let last_alerted = self.alerted_windows.get(title).copied();
            if last_alerted.map(|t| now - t < re_alert_gap).unwrap_or(false) {
                continue;
            }

            let win_info = snapshot
                .visible_windows
                .iter()
                .chain(snapshot.active_window.iter())
                .find(|w| &w.title == title);

            if let Some(win) = win_info {
                let doc = win.document_hint.clone().unwrap_or_else(|| title.clone());
                let idle_mins = (now - idle_since).num_minutes().max(0) as u64;
                self.alerted_windows.insert(title.clone(), now);
                alerts.push(AlertCondition {
                    kind: AlertKind::DocumentOpenUnused,
                    description: format!(
                        "'{}' in {} has been open and unused for {}.",
                        doc,
                        win.app_name,
                        format_duration(idle_mins * 60),
                    ),
                    process_name: None,
                    pid: win.pid,
                    document: Some(doc),
                    app_name: Some(win.app_name.clone()),
                    duration_mins: idle_mins,
                    cpu_percent: None,
                    memory_mb: None,
                });
            }
        }

        alerts
    }

    pub fn daily_log_path(&self, date: NaiveDate) -> PathBuf {
        self.activity_dir.join(format!("{}.md", date))
    }

    pub fn live_log_path(&self) -> PathBuf {
        self.activity_dir.join("LIVE.md")
    }
}

fn is_system_process(name: &str) -> bool {
    let lower = name.to_lowercase();
    const SYSTEM_NAMES: &[&str] = &[
        "system", "kernel", "kthreadd", "kworker", "ksoftirqd",
        "migration", "idle", "svchost", "csrss", "wininit", "services",
        "lsass", "smss", "dwm", "winlogon", "explorer", "taskhostw",
        "launchd", "kernel_task", "logd", "cfprefsd", "coreaudiod",
        "systemd", "dbus-daemon", "pulseaudio", "Xorg", "x11",
        "panther",
    ];
    SYSTEM_NAMES.iter().any(|s| lower.contains(s))
}

fn format_duration(secs: u64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    let s = secs % 60;

    if days > 0 {
        format!("{}d {}h {}m", days, hours, mins)
    } else if hours > 0 {
        format!("{}h {}m", hours, mins)
    } else if mins > 0 {
        format!("{}m {}s", mins, s)
    } else {
        format!("{}s", s)
    }
}

fn escape_md_cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}
