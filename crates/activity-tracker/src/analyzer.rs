use std::path::PathBuf;
use chrono::{DateTime, Duration, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};

pub struct ActivityAnalyzer {
    activity_dir: PathBuf,
}

impl ActivityAnalyzer {
    pub fn new(activity_dir: PathBuf) -> Self {
        Self { activity_dir }
    }

    pub async fn get_current_state(&self) -> String {
        let path = self.activity_dir.join("LIVE.md");
        match tokio::fs::read_to_string(&path).await {
            Ok(content) if !content.trim().is_empty() => content,
            _ => "No live activity data available. Activity tracking may not have started yet.".to_string(),
        }
    }

    pub async fn query_time_range(&self, start: DateTime<Utc>, end: DateTime<Utc>) -> String {
        let mut relevant_dates: Vec<NaiveDate> = Vec::new();
        let mut cursor = start.with_timezone(&Local).date_naive();
        let end_date = end.with_timezone(&Local).date_naive();

        while cursor <= end_date {
            relevant_dates.push(cursor);
            cursor = cursor.succ_opt().unwrap_or(cursor);
        }

        let mut sections: Vec<String> = Vec::new();

        for date in relevant_dates {
            let path = self.activity_dir.join(format!("{}.md", date));
            let raw = match tokio::fs::read_to_string(&path).await {
                Ok(r) => r,
                Err(_) => continue,
            };

            let filtered = filter_log_to_range(&raw, date, start, end);
            if !filtered.trim().is_empty() {
                sections.push(filtered);
            }
        }

        if sections.is_empty() {
            return format!(
                "No activity recorded between {} and {}.",
                start.with_timezone(&Local).format("%H:%M on %d %b"),
                end.with_timezone(&Local).format("%H:%M on %d %b")
            );
        }

        sections.join("\n\n---\n\n")
    }

    pub async fn daily_summary(&self, date: NaiveDate) -> String {
        let path = self.activity_dir.join(format!("{}.md", date));
        match tokio::fs::read_to_string(&path).await {
            Ok(content) if !content.trim().is_empty() => content,
            _ => format!("No activity log found for {}.", date),
        }
    }

    pub async fn last_n_hours(&self, hours: u64) -> String {
        let end = Utc::now();
        let start = end - Duration::hours(hours as i64);
        self.query_time_range(start, end).await
    }

    pub async fn today_summary(&self) -> String {
        let today = Local::now().date_naive();
        self.daily_summary(today).await
    }

    pub async fn list_available_dates(&self) -> Vec<NaiveDate> {
        let mut dates = Vec::new();
        let mut entries = match tokio::fs::read_dir(&self.activity_dir).await {
            Ok(e) => e,
            Err(_) => return dates,
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.ends_with(".md") && name_str != "LIVE.md" {
                let stem = name_str.trim_end_matches(".md");
                if let Ok(date) = NaiveDate::parse_from_str(stem, "%Y-%m-%d") {
                    dates.push(date);
                }
            }
        }

        dates.sort_unstable_by(|a, b| b.cmp(a));
        dates
    }
}

fn filter_log_to_range(
    content: &str,
    date: NaiveDate,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> String {
    let mut in_sessions_table = false;
    let mut result_header: Option<String> = None;
    let mut table_rows: Vec<String> = Vec::new();
    let mut detail_blocks: Vec<String> = Vec::new();
    let mut current_detail: Option<(NaiveDateTime, Vec<String>)> = None;

    for line in content.lines() {
        if line.starts_with("## Sessions") {
            in_sessions_table = true;
            continue;
        }

        if in_sessions_table {
            if line.starts_with("## ") && !line.starts_with("## Sessions") {
                in_sessions_table = false;
            } else if line.starts_with("| Start") || line.starts_with("|---") {
                continue;
            } else if line.starts_with('|') {
                let cols: Vec<&str> = line.split('|').map(|s| s.trim()).collect();
                if cols.len() >= 4 {
                    let time_str = cols[1];
                    if let Ok(naive_time) = chrono::NaiveTime::parse_from_str(time_str, "%H:%M:%S") {
                        let naive_dt = date.and_time(naive_time);
                        if let Some(local_dt) = Local.from_local_datetime(&naive_dt).earliest() {
                            let utc_dt = local_dt.with_timezone(&Utc);
                            if utc_dt >= start && utc_dt <= end {
                                table_rows.push(line.to_string());
                            }
                        }
                    }
                }
            }
        }

        if line.starts_with("### [") {
            if let Some((_, lines)) = current_detail.take() {
                detail_blocks.push(lines.join("\n"));
            }

            let time_part = line
                .trim_start_matches("### [")
                .split(']')
                .next()
                .unwrap_or("");

            if let Ok(naive_time) = chrono::NaiveTime::parse_from_str(time_part, "%H:%M:%S") {
                let naive_dt = date.and_time(naive_time);
                if let Some(local_dt) = Local.from_local_datetime(&naive_dt).earliest() {
                    let utc_dt = local_dt.with_timezone(&Utc);
                    if utc_dt >= start && utc_dt <= end {
                        current_detail = Some((naive_dt, vec![line.to_string()]));
                    } else {
                        current_detail = None;
                    }
                }
            }
            continue;
        }

        if let Some((_, ref mut lines)) = current_detail {
            lines.push(line.to_string());
        }

        if line.starts_with("# Activity Log") {
            result_header = Some(line.to_string());
        }
    }

    if let Some((_, lines)) = current_detail.take() {
        detail_blocks.push(lines.join("\n"));
    }

    if table_rows.is_empty() && detail_blocks.is_empty() {
        return String::new();
    }

    let mut output = result_header
        .unwrap_or_else(|| format!("# Activity Log — {}", date))
        + "\n\n";

    if !table_rows.is_empty() {
        output.push_str("## Sessions\n\n| Start | End | Duration | Application | Document |\n|-------|-----|----------|-------------|----------|\n");
        for row in &table_rows {
            output.push_str(row);
            output.push('\n');
        }
        output.push('\n');
    }

    for block in &detail_blocks {
        output.push_str(block);
        output.push_str("\n\n");
    }

    output
}
