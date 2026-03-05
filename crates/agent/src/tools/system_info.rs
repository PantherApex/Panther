use std::pin::Pin;
use std::future::Future;
use serde_json::json;
use tokio::process::Command;

use super::Tool;

pub struct SystemInfoTool;

async fn run_silent(cmd: &str, args: &[&str]) -> Option<String> {
    Command::new(cmd)
        .args(args)
        .output()
        .await
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

async fn which(cmd: &str) -> bool {
    if cfg!(target_os = "windows") {
        Command::new("where").arg(cmd).output().await.map(|o| o.status.success()).unwrap_or(false)
    } else {
        Command::new("which").arg(cmd).output().await.map(|o| o.status.success()).unwrap_or(false)
    }
}

async fn gather_system_info() -> String {
    let mut parts: Vec<String> = Vec::new();

    let os_name = if cfg!(target_os = "macos") { "macOS" }
                  else if cfg!(target_os = "windows") { "Windows" }
                  else { "Linux" };

    parts.push(format!("OS: {}", os_name));

    if cfg!(target_os = "macos") {
        if let Some(v) = run_silent("sw_vers", &["-productVersion"]).await {
            parts.push(format!("macOS version: {}", v));
        }
        if let Some(info) = run_silent("sysctl", &["-n", "machdep.cpu.brand_string"]).await {
            parts.push(format!("CPU: {}", info));
        }
        if let Some(mem) = run_silent("sysctl", &["-n", "hw.memsize"]).await {
            if let Ok(bytes) = mem.parse::<u64>() {
                parts.push(format!("RAM: {} GB", bytes / 1_073_741_824));
            }
        }
        if let Some(disk) = run_silent("df", &["-h", "/"]).await {
            let lines: Vec<&str> = disk.lines().collect();
            if lines.len() > 1 {
                parts.push(format!("Disk (/):\n{}", lines[1]));
            }
        }
        if let Some(bat) = run_silent("pmset", &["-g", "batt"]).await {
            let line = bat.lines().nth(1).unwrap_or("").trim().to_string();
            if !line.is_empty() {
                parts.push(format!("Battery: {}", line));
            }
        }
        if let Some(wifi) = run_silent("networksetup", &["-getairportnetwork", "en0"]).await {
            parts.push(format!("WiFi: {}", wifi));
        }
        if let Some(vol) = run_silent("osascript", &["-e", "output volume of (get volume settings)"]).await {
            parts.push(format!("Volume: {}%", vol));
        }
        if let Some(bright) = run_silent("osascript", &["-e", "tell application \"System Events\" to get brightness of first display"]).await {
            parts.push(format!("Screen brightness: {}", bright));
        }
    } else if cfg!(target_os = "windows") {
        let win_ver = run_silent("powershell", &["-NoProfile", "-NonInteractive", "-Command",
            "(Get-WmiObject -Class Win32_OperatingSystem).Caption"]).await;
        if let Some(v) = win_ver {
            parts.push(format!("Windows version: {}", v));
        }
        let cpu = run_silent("powershell", &["-NoProfile", "-NonInteractive", "-Command",
            "(Get-WmiObject Win32_Processor).Name"]).await;
        if let Some(c) = cpu {
            parts.push(format!("CPU: {}", c));
        }
        let ram = run_silent("powershell", &["-NoProfile", "-NonInteractive", "-Command",
            "[math]::Round((Get-WmiObject Win32_ComputerSystem).TotalPhysicalMemory/1GB, 1)"]).await;
        if let Some(r) = ram {
            parts.push(format!("RAM: {} GB", r));
        }
        let disk = run_silent("powershell", &["-NoProfile", "-NonInteractive", "-Command",
            "Get-PSDrive C | Select-Object Used,Free | ForEach-Object { \"Used: $([math]::Round($_.Used/1GB,1))GB Free: $([math]::Round($_.Free/1GB,1))GB\" }"]).await;
        if let Some(d) = disk {
            parts.push(format!("Disk (C:): {}", d));
        }
        let bat = run_silent("powershell", &["-NoProfile", "-NonInteractive", "-Command",
            "(Get-WmiObject Win32_Battery).EstimatedChargeRemaining"]).await;
        if let Some(b) = bat {
            parts.push(format!("Battery: {}%", b));
        }
    } else {
        if let Some(distro) = run_silent("cat", &["/etc/os-release"]).await {
            let name_line = distro.lines()
                .find(|l| l.starts_with("PRETTY_NAME="))
                .map(|l| l.trim_start_matches("PRETTY_NAME=").trim_matches('"').to_string());
            if let Some(name) = name_line {
                parts.push(format!("Distro: {}", name));
            }
        }
        if let Some(kern) = run_silent("uname", &["-r"]).await {
            parts.push(format!("Kernel: {}", kern));
        }
        if let Some(cpu) = run_silent("sh", &["-c", "grep 'model name' /proc/cpuinfo | head -1 | cut -d: -f2"]).await {
            parts.push(format!("CPU: {}", cpu.trim()));
        }
        if let Some(mem) = run_silent("sh", &["-c", "free -h | awk '/^Mem:/{print $2\" total, \"$3\" used, \"$4\" free\"}'"]).await {
            parts.push(format!("RAM: {}", mem));
        }
        if let Some(disk) = run_silent("df", &["-h", "/"]).await {
            let lines: Vec<&str> = disk.lines().collect();
            if lines.len() > 1 {
                parts.push(format!("Disk (/):\n{}", lines[1]));
            }
        }
        if which("upower").await {
            if let Some(bat) = run_silent("sh", &["-c", "upower -i $(upower -e | grep battery) | grep -E 'percentage|state'"]).await {
                parts.push(format!("Battery:\n{}", bat));
            }
        } else if let Some(bat) = run_silent("cat", &["/sys/class/power_supply/BAT0/capacity"]).await {
            if let Some(status) = run_silent("cat", &["/sys/class/power_supply/BAT0/status"]).await {
                parts.push(format!("Battery: {}% ({})", bat, status));
            }
        }
        if which("nmcli").await {
            if let Some(net) = run_silent("nmcli", &["-t", "-f", "NAME,TYPE,STATE", "connection", "show", "--active"]).await {
                parts.push(format!("Network connections:\n{}", net));
            }
        } else if let Some(ip) = run_silent("sh", &["-c", "ip route get 8.8.8.8 2>/dev/null | head -1"]).await {
            parts.push(format!("Network: {}", ip));
        }
        if let Some(display) = std::env::var("DISPLAY").ok().or_else(|| std::env::var("WAYLAND_DISPLAY").ok()) {
            parts.push(format!("Display: {}", display));
        }
    }

    if let Some(uptime) = run_silent("uptime", &[]).await {
        parts.push(format!("Uptime: {}", uptime.trim()));
    }

    if let Some(home) = dirs::home_dir() {
        parts.push(format!("Home dir: {}", home.display()));
    }

    let hostname = run_silent("hostname", &[]).await.unwrap_or_else(|| "unknown".to_string());
    parts.push(format!("Hostname: {}", hostname));

    parts.join("\n")
}

async fn list_running_processes() -> String {
    if cfg!(target_os = "macos") {
        run_silent("sh", &["-c", "ps aux | sort -rk 3 | head -20"]).await
            .unwrap_or_else(|| "ps not available".to_string())
    } else if cfg!(target_os = "windows") {
        run_silent("powershell", &["-NoProfile", "-NonInteractive", "-Command",
            "Get-Process | Sort-Object CPU -Descending | Select-Object -First 20 Name,CPU,WorkingSet | Format-Table -AutoSize"]).await
            .unwrap_or_else(|| "Get-Process not available".to_string())
    } else {
        run_silent("sh", &["-c", "ps aux --sort=-%cpu | head -20"]).await
            .unwrap_or_else(|| "ps not available".to_string())
    }
}

async fn list_open_windows() -> String {
    if cfg!(target_os = "macos") {
        run_silent("osascript", &["-e",
            "tell application \"System Events\" to get name of every process whose background only is false"]).await
            .unwrap_or_else(|| "osascript not available".to_string())
    } else if cfg!(target_os = "windows") {
        run_silent("powershell", &["-NoProfile", "-NonInteractive", "-Command",
            "Get-Process | Where-Object {$_.MainWindowTitle -ne ''} | Select-Object ProcessName,MainWindowTitle | Format-Table -AutoSize"]).await
            .unwrap_or_else(|| "Get-Process not available".to_string())
    } else {
        if Command::new("which").arg("wmctrl").output().await.map(|o| o.status.success()).unwrap_or(false) {
            run_silent("wmctrl", &["-l"]).await
                .unwrap_or_else(|| "wmctrl not available".to_string())
        } else if Command::new("which").arg("xdotool").output().await.map(|o| o.status.success()).unwrap_or(false) {
            run_silent("sh", &["-c", "xdotool search --onlyvisible --name '' 2>/dev/null | head -20 | xargs -I{} xdotool getwindowname {}"]).await
                .unwrap_or_else(|| "xdotool failed".to_string())
        } else {
            "Install wmctrl or xdotool to list windows on Linux".to_string()
        }
    }
}

async fn get_display_info() -> String {
    if cfg!(target_os = "macos") {
        run_silent("system_profiler", &["SPDisplaysDataType"]).await
            .unwrap_or_else(|| "system_profiler not available".to_string())
    } else if cfg!(target_os = "windows") {
        run_silent("powershell", &["-NoProfile", "-NonInteractive", "-Command",
            "Get-WmiObject Win32_VideoController | Select-Object Name,CurrentHorizontalResolution,CurrentVerticalResolution | Format-Table -AutoSize"]).await
            .unwrap_or_else(|| "Get-WmiObject not available".to_string())
    } else {
        if Command::new("which").arg("xrandr").output().await.map(|o| o.status.success()).unwrap_or(false) {
            run_silent("xrandr", &["--current"]).await
                .unwrap_or_else(|| "xrandr not available".to_string())
        } else {
            run_silent("sh", &["-c", "cat /sys/class/drm/*/status 2>/dev/null"]).await
                .unwrap_or_else(|| "No display info tool available".to_string())
        }
    }
}

async fn get_network_info() -> String {
    if cfg!(target_os = "macos") {
        let ifaces = run_silent("ifconfig", &[]).await.unwrap_or_default();
        let wifi = run_silent("networksetup", &["-getairportnetwork", "en0"]).await.unwrap_or_default();
        format!("{}\nWiFi: {}", ifaces, wifi)
    } else if cfg!(target_os = "windows") {
        run_silent("powershell", &["-NoProfile", "-NonInteractive", "-Command",
            "Get-NetIPAddress | Where-Object {$_.AddressFamily -eq 'IPv4'} | Select-Object InterfaceAlias,IPAddress | Format-Table -AutoSize"]).await
            .unwrap_or_else(|| "Get-NetIPAddress not available".to_string())
    } else {
        let ip_result = run_silent("ip", &["addr", "show"]).await;
        if let Some(info) = ip_result {
            info
        } else {
            run_silent("ifconfig", &[]).await
                .unwrap_or_else(|| "ip/ifconfig not available".to_string())
        }
    }
}

impl Tool for SystemInfoTool {
    fn name(&self) -> &str { "system_info" }

    fn description(&self) -> &str {
        "Get detailed information about this computer: hardware specs, OS, battery, network, running processes, open windows, display info, storage. Use for system diagnostics or when asked about computer state."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "enum": ["overview", "processes", "windows", "display", "network", "all"],
                    "description": "What to query: 'overview' for basic system info, 'processes' for running apps, 'windows' for open windows, 'display' for screen info, 'network' for network interfaces, 'all' for everything"
                }
            },
            "required": ["query"]
        })
    }

    fn execute<'a>(&'a self, args: serde_json::Value) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
        Box::pin(async move {
            let query = match args.get("query").and_then(|v| v.as_str()) {
                Some(q) => q.to_string(),
                None => "overview".to_string(),
            };

            match query.as_str() {
                "overview" => gather_system_info().await,
                "processes" => list_running_processes().await,
                "windows" => list_open_windows().await,
                "display" => get_display_info().await,
                "network" => get_network_info().await,
                "all" => {
                    let (overview, procs, wins, display, net) = tokio::join!(
                        gather_system_info(),
                        list_running_processes(),
                        list_open_windows(),
                        get_display_info(),
                        get_network_info(),
                    );
                    format!(
                        "=== System Overview ===\n{}\n\n=== Running Processes ===\n{}\n\n=== Open Windows ===\n{}\n\n=== Display ===\n{}\n\n=== Network ===\n{}",
                        overview, procs, wins, display, net
                    )
                }
                other => format!("Unknown query: '{}'. Use: overview, processes, windows, display, network, all", other),
            }
        })
    }
}
