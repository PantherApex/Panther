use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sysinfo::System;
use tokio::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowInfo {
    pub title: String,
    pub app_name: String,
    pub document_hint: Option<String>,
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub cpu_percent: f32,
    pub memory_mb: u64,
    pub elapsed_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivitySnapshot {
    pub captured_at: DateTime<Utc>,
    pub active_window: Option<WindowInfo>,
    pub visible_windows: Vec<WindowInfo>,
    pub top_processes: Vec<ProcessInfo>,
}

pub struct ActivityMonitor;

impl ActivityMonitor {
    pub async fn capture() -> ActivitySnapshot {
        let now = Utc::now();
        let (active_window, visible_windows) = tokio::join!(
            capture_active_window(),
            capture_visible_windows(),
        );
        let top_processes = collect_top_processes();

        ActivitySnapshot {
            captured_at: now,
            active_window,
            visible_windows,
            top_processes,
        }
    }
}

fn collect_top_processes() -> Vec<ProcessInfo> {
    let mut sys = System::new_all();
    sys.refresh_all();

    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let mut processes: Vec<ProcessInfo> = sys
        .processes()
        .iter()
        .map(|(pid, proc)| {
            let started = proc.start_time();
            let elapsed = if started > 0 && now_unix >= started {
                now_unix - started
            } else {
                0
            };
            ProcessInfo {
                pid: pid.as_u32(),
                name: proc.name().to_string(),
                cpu_percent: proc.cpu_usage(),
                memory_mb: proc.memory() / 1_048_576,
                elapsed_secs: elapsed,
            }
        })
        .filter(|p| !p.name.is_empty())
        .collect();

    processes.sort_by(|a, b| b.cpu_percent.partial_cmp(&a.cpu_percent).unwrap_or(std::cmp::Ordering::Equal));
    processes.truncate(50);
    processes
}

async fn run_cmd(program: &str, args: &[&str]) -> Option<String> {
    Command::new(program)
        .args(args)
        .output()
        .await
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(target_os = "windows")]
async fn capture_active_window() -> Option<WindowInfo> {
    let script = concat!(
        "try {",
        " try { $null = [PantherFW.W32]::GetForegroundWindow() }",
        " catch {",
        "   $s = '[DllImport(\"user32.dll\")] public static extern IntPtr GetForegroundWindow();",
        "    [DllImport(\"user32.dll\",CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h,System.Text.StringBuilder b,int n);",
        "    [DllImport(\"user32.dll\")] public static extern uint GetWindowThreadProcessId(IntPtr h,out uint p);';",
        "   Add-Type -MemberDefinition $s -Name W32 -Namespace PantherFW -EA SilentlyContinue",
        " }",
        " $h=[PantherFW.W32]::GetForegroundWindow();",
        " $sb=New-Object System.Text.StringBuilder 512;",
        " [void][PantherFW.W32]::GetWindowText($h,$sb,512);",
        " $p=0;",
        " [void][PantherFW.W32]::GetWindowThreadProcessId($h,[ref]$p);",
        " $proc=Get-Process -Id $p -EA 0;",
        " @{title=$sb.ToString();app=if($proc){$proc.ProcessName}else{'unknown'};pid=$p}|ConvertTo-Json -Compress",
        "} catch { '{}' }"
    );

    let raw = run_cmd("powershell", &["-NoProfile", "-NonInteractive", "-Command", script]).await?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let title = v["title"].as_str()?.to_string();
    if title.is_empty() {
        return None;
    }
    let app_name = v["app"].as_str().unwrap_or("unknown").to_string();
    let pid = v["pid"].as_u64().map(|p| p as u32);
    let document_hint = extract_document_hint(&title, &app_name);
    Some(WindowInfo { title, app_name, document_hint, pid })
}

#[cfg(target_os = "windows")]
async fn capture_visible_windows() -> Vec<WindowInfo> {
    let script = "Get-Process | Where-Object {$_.MainWindowTitle -ne ''} | Select-Object ProcessName,MainWindowTitle,Id | ConvertTo-Json -Compress";
    let raw = match run_cmd("powershell", &["-NoProfile", "-NonInteractive", "-Command", script]).await {
        Some(r) => r,
        None => return Vec::new(),
    };

    let arr: Vec<serde_json::Value> = if raw.starts_with('[') {
        serde_json::from_str(&raw).unwrap_or_default()
    } else if raw.starts_with('{') {
        serde_json::from_str::<serde_json::Value>(&raw)
            .ok()
            .map(|v| vec![v])
            .unwrap_or_default()
    } else {
        return Vec::new();
    };

    arr.into_iter()
        .filter_map(|v| {
            let title = v["MainWindowTitle"].as_str()?.to_string();
            let app = v["ProcessName"].as_str().unwrap_or("unknown").to_string();
            let pid = v["Id"].as_u64().map(|p| p as u32);
            let doc = extract_document_hint(&title, &app);
            Some(WindowInfo { title, app_name: app, document_hint: doc, pid })
        })
        .collect()
}

#[cfg(target_os = "macos")]
async fn capture_active_window() -> Option<WindowInfo> {
    let script = r#"
tell application "System Events"
    set fp to first process whose frontmost is true
    set appName to name of fp
    set appPid to unix id of fp
    set winTitle to ""
    try
        set winTitle to name of first window of fp
    end try
    return appName & "|||" & appPid & "|||" & winTitle
end tell
"#;

    let raw = run_cmd("osascript", &["-e", script]).await?;
    let parts: Vec<&str> = raw.splitn(3, "|||").collect();
    if parts.is_empty() {
        return None;
    }
    let app_name = parts[0].trim().to_string();
    let pid = parts.get(1).and_then(|s| s.trim().parse::<u32>().ok());
    let win_title = parts.get(2).map(|s| s.trim().to_string()).unwrap_or_default();
    let title = if win_title.is_empty() { app_name.clone() } else { win_title.clone() };
    let document_hint = if win_title.is_empty() {
        None
    } else {
        extract_document_hint(&win_title, &app_name)
    };
    Some(WindowInfo { title, app_name, document_hint, pid })
}

#[cfg(target_os = "macos")]
async fn capture_visible_windows() -> Vec<WindowInfo> {
    let script = r#"
tell application "System Events"
    set result to {}
    set procs to every process whose background only is false
    repeat with p in procs
        set appName to name of p
        try
            set wins to every window of p
            repeat with w in wins
                set winName to name of w
                if winName is not "" then
                    set end of result to (appName & "|||" & winName)
                end if
            end repeat
        end try
    end repeat
    return result
end tell
"#;

    let raw = match run_cmd("osascript", &["-e", script]).await {
        Some(r) => r,
        None => return Vec::new(),
    };

    raw.split(", ")
        .filter(|s| s.contains("|||"))
        .filter_map(|entry| {
            let mut parts = entry.splitn(2, "|||");
            let app = parts.next()?.trim().to_string();
            let title = parts.next()?.trim().to_string();
            let doc = extract_document_hint(&title, &app);
            Some(WindowInfo { title, app_name: app, document_hint: doc, pid: None })
        })
        .collect()
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
async fn capture_active_window() -> Option<WindowInfo> {
    let title = run_cmd("sh", &["-c", "xdotool getactivewindow getwindowname 2>/dev/null"]).await?;
    let pid_str = run_cmd("sh", &["-c", "xdotool getactivewindow getwindowpid 2>/dev/null"]).await;
    let pid = pid_str.and_then(|s| s.trim().parse::<u32>().ok());

    let app_name = if let Some(p) = pid {
        run_cmd("sh", &["-c", &format!("cat /proc/{}/comm 2>/dev/null", p)])
            .await
            .unwrap_or_else(|| "unknown".to_string())
    } else {
        "unknown".to_string()
    };

    let document_hint = extract_document_hint(&title, &app_name);
    Some(WindowInfo { title, app_name, document_hint, pid })
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
async fn capture_visible_windows() -> Vec<WindowInfo> {
    let raw = match run_cmd("sh", &["-c", "wmctrl -l -p 2>/dev/null || xdotool search --onlyvisible --name '' 2>/dev/null | xargs -I{} xdotool getwindowname {} 2>/dev/null | head -30"]).await {
        Some(r) => r,
        None => return Vec::new(),
    };

    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let title = if line.contains(' ') {
                line.splitn(5, ' ').nth(4).unwrap_or(line).trim().to_string()
            } else {
                line.trim().to_string()
            };
            let doc = extract_document_hint(&title, "unknown");
            WindowInfo {
                title,
                app_name: "unknown".to_string(),
                document_hint: doc,
                pid: None,
            }
        })
        .collect()
}

static KNOWN_EXTENSIONS: &[&str] = &[
    ".pdf", ".docx", ".doc", ".xlsx", ".xls", ".pptx", ".ppt",
    ".txt", ".md", ".rs", ".py", ".js", ".ts", ".html", ".htm", ".css",
    ".json", ".toml", ".yaml", ".yml", ".go", ".java", ".c", ".cpp",
    ".h", ".hpp", ".rb", ".php", ".sh", ".bat", ".ps1", ".sql",
    ".csv", ".xml", ".swift", ".kt", ".dart", ".vue", ".jsx", ".tsx",
    ".png", ".jpg", ".jpeg", ".gif", ".svg", ".mp4", ".mp3", ".mov",
    ".zip", ".tar", ".gz", ".7z", ".rar",
];

static SEPARATOR_PATTERNS: &[&str] = &[" - ", " — ", " – ", " | "];

fn extract_document_hint(title: &str, _app_name: &str) -> Option<String> {
    let lower = title.to_lowercase();

    for ext in KNOWN_EXTENSIONS {
        if let Some(pos) = lower.find(ext) {
            let end = pos + ext.len();
            let start = title[..pos]
                .rfind(|c: char| c == '/' || c == '\\')
                .map(|i| i + 1)
                .unwrap_or(0);
            let candidate = &title[start..end];
            if !candidate.is_empty() && candidate.len() <= 200 {
                return Some(candidate.to_string());
            }
        }
    }

    for sep in SEPARATOR_PATTERNS {
        if let Some(pos) = title.rfind(sep) {
            let before = title[..pos].trim();
            if !before.is_empty() && before.len() < 120 {
                return Some(before.to_string());
            }
        }
    }

    None
}
