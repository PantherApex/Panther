use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use serde_json::json;
use tokio::process::Command;
use tokio::sync::Mutex;

use super::Tool;

pub struct CaptureMediaTool {
    temp_dir: PathBuf,
    turn_cache: Arc<Mutex<HashMap<String, PathBuf>>>,
}

impl CaptureMediaTool {
    pub fn new(temp_dir: PathBuf) -> Self {
        Self {
            temp_dir,
            turn_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn reset_turn(&self) {
        self.turn_cache.lock().await.clear();
    }
}

async fn probe(cmd: &str) -> bool {
    if cfg!(target_os = "windows") {
        Command::new("cmd")
            .args(["/C", &format!("where {}", cmd)])
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
    } else {
        Command::new("which")
            .arg(cmd)
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

async fn run_cmd(mut cmd: Command, timeout_secs: u64) -> Result<std::process::Output, String> {
    tokio::time::timeout(Duration::from_secs(timeout_secs), cmd.output())
        .await
        .map_err(|_| format!("timed out after {}s", timeout_secs))?
        .map_err(|e| format!("failed to launch: {}", e))
}

async fn powershell_run(script: &str, timeout_secs: u64) -> Result<String, String> {
    let mut cmd = Command::new("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script]);
    let out = run_cmd(cmd, timeout_secs).await?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if out.status.success() {
        Ok(stdout)
    } else {
        Err(if stderr.is_empty() { stdout } else { stderr })
    }
}

async fn ffmpeg_dshow_devices(want_video: bool) -> Vec<String> {
    let out = tokio::time::timeout(
        Duration::from_secs(8),
        Command::new("ffmpeg")
            .args(["-hide_banner", "-list_devices", "true", "-f", "dshow", "-i", "dummy"])
            .output(),
    )
    .await
    .ok()
    .and_then(|r| r.ok());

    let Some(out) = out else { return Vec::new() };
    let combined = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));

    let video_marker = "DirectShow video devices";
    let audio_marker = "DirectShow audio devices";
    let mut in_section = false;
    let mut devices = Vec::new();

    for line in combined.lines() {
        if line.contains(video_marker) {
            in_section = want_video;
            continue;
        }
        if line.contains(audio_marker) {
            in_section = !want_video;
            continue;
        }
        if in_section {
            if let Some(start) = line.find('"') {
                let rest = &line[start + 1..];
                if let Some(end) = rest.find('"') {
                    let name = rest[..end].trim().to_string();
                    if !name.is_empty() && !name.starts_with('@') {
                        devices.push(name);
                    }
                }
            }
        }
    }
    devices
}

async fn capture_screenshot_windows(out_path: &PathBuf) -> Result<(), String> {
    let path_str = out_path.to_string_lossy().to_string();
    let mut errors: Vec<String> = Vec::new();

    if probe("ffmpeg").await {
        let mut cmd = Command::new("ffmpeg");
        cmd.args([
            "-y", "-hide_banner", "-loglevel", "error",
            "-f", "gdigrab", "-framerate", "1", "-i", "desktop",
            "-vframes", "1", &path_str,
        ]);
        match run_cmd(cmd, 20).await {
            Ok(_) if out_path.exists() => return Ok(()),
            Ok(out) => errors.push(format!("ffmpeg gdigrab: {}", String::from_utf8_lossy(&out.stderr).trim())),
            Err(e) => errors.push(format!("ffmpeg gdigrab: {}", e)),
        }
    }

    let ps_forms = r#"Add-Type -AssemblyName System.Windows.Forms,System.Drawing
$p = $env:PANTHER_OUT
$screens = [System.Windows.Forms.Screen]::AllScreens
$l = ($screens | ForEach-Object { $_.Bounds.Left }  | Measure-Object -Minimum).Minimum
$t = ($screens | ForEach-Object { $_.Bounds.Top }   | Measure-Object -Minimum).Minimum
$r = ($screens | ForEach-Object { $_.Bounds.Right }  | Measure-Object -Maximum).Maximum
$b = ($screens | ForEach-Object { $_.Bounds.Bottom } | Measure-Object -Maximum).Maximum
$w = $r - $l; $h = $b - $t
$bmp = New-Object System.Drawing.Bitmap($w, $h)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($l, $t, 0, 0, [System.Drawing.Size]::new($w, $h))
$bmp.Save($p, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()"#;

    {
        let mut cmd = tokio::process::Command::new("powershell");
        cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", ps_forms]);
        cmd.env("PANTHER_OUT", &path_str);
        match run_cmd(cmd, 20).await {
            Ok(_) if out_path.exists() => return Ok(()),
            Ok(out) => {
                let e = String::from_utf8_lossy(&out.stderr).trim().to_string();
                errors.push(format!("ps-forms: {}", if e.is_empty() { "file not created".into() } else { e }));
            }
            Err(e) => errors.push(format!("ps-forms: {}", e)),
        }
    }

    let ps_gdi = r#"Add-Type -AssemblyName System.Drawing
Add-Type -TypeDefinition @"
using System; using System.Drawing; using System.Drawing.Imaging; using System.Runtime.InteropServices;
public class PantherGdi {
    [DllImport("user32.dll")] public static extern IntPtr GetDC(IntPtr h);
    [DllImport("user32.dll")] public static extern int ReleaseDC(IntPtr h, IntPtr dc);
    [DllImport("user32.dll")] public static extern int GetSystemMetrics(int n);
    [DllImport("gdi32.dll")] public static extern IntPtr CreateCompatibleDC(IntPtr dc);
    [DllImport("gdi32.dll")] public static extern IntPtr CreateCompatibleBitmap(IntPtr dc, int w, int h);
    [DllImport("gdi32.dll")] public static extern IntPtr SelectObject(IntPtr dc, IntPtr o);
    [DllImport("gdi32.dll")] public static extern bool BitBlt(IntPtr d,int x,int y,int w,int h,IntPtr s,int sx,int sy,uint op);
    [DllImport("gdi32.dll")] public static extern bool DeleteDC(IntPtr dc);
    [DllImport("gdi32.dll")] public static extern bool DeleteObject(IntPtr o);
    public static void Snap(string path) {
        int ox = GetSystemMetrics(76); int oy = GetSystemMetrics(77);
        int w  = GetSystemMetrics(78); int h  = GetSystemMetrics(79);
        IntPtr src = GetDC(IntPtr.Zero);
        IntPtr dst = CreateCompatibleDC(src);
        IntPtr bmp = CreateCompatibleBitmap(src, w, h);
        SelectObject(dst, bmp);
        BitBlt(dst, 0, 0, w, h, src, ox, oy, 0x00CC0020u);
        SelectObject(dst, IntPtr.Zero);
        DeleteDC(dst); ReleaseDC(IntPtr.Zero, src);
        using (var img = Image.FromHbitmap(bmp)) { img.Save(path, ImageFormat.Png); }
        DeleteObject(bmp);
    }
}
"@ -ReferencedAssemblies System.Drawing
[PantherGdi]::Snap($env:PANTHER_OUT)"#;

    {
        let mut cmd = tokio::process::Command::new("powershell");
        cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", ps_gdi]);
        cmd.env("PANTHER_OUT", &path_str);
        match run_cmd(cmd, 25).await {
            Ok(_) if out_path.exists() => return Ok(()),
            Ok(out) => {
                let e = String::from_utf8_lossy(&out.stderr).trim().to_string();
                errors.push(format!("ps-gdi32: {}", if e.is_empty() { "file not created".into() } else { e }));
            }
            Err(e) => errors.push(format!("ps-gdi32: {}", e)),
        }
    }

    Err(format!("Screenshot failed on all backends — {}. Install ffmpeg (winget install Gyan.FFmpeg) to guarantee capture.", errors.join(" | ")))
}

async fn capture_screenshot_macos(out_path: &PathBuf) -> Result<(), String> {
    let path_str = out_path.to_string_lossy().to_string();
    let mut cmd = Command::new("screencapture");
    cmd.args(["-x", "-t", "png", &path_str]);
    let out = run_cmd(cmd, 10).await?;
    if out_path.exists() { return Ok(()); }
    Err(format!("screencapture failed: {}", String::from_utf8_lossy(&out.stderr)))
}

async fn capture_screenshot_linux(out_path: &PathBuf) -> Result<(), String> {
    let path_str = out_path.to_string_lossy().to_string();
    let mut errors = Vec::<String>::new();

    macro_rules! try_cmd {
        ($cmd:expr, $args:expr) => {{
            if probe($cmd).await {
                let mut c = Command::new($cmd);
                for a in $args { c.arg(a); }
                if let Ok(out) = run_cmd(c, 12).await {
                    if out_path.exists() { return Ok(()); }
                    let e = String::from_utf8_lossy(&out.stderr).to_string();
                    if !e.is_empty() { errors.push(format!("{}: {}", $cmd, e)); }
                }
            }
        }};
    }

    try_cmd!("gnome-screenshot", ["-f", &path_str]);
    try_cmd!("scrot", [path_str.as_str()]);
    try_cmd!("spectacle", ["-b", "-n", "-o", &path_str]);

    if probe("flameshot").await {
        let parent = out_path.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|| "/tmp".to_string());
        let mut c = Command::new("flameshot");
        c.args(["full", "-p", &parent]);
        if let Ok(_) = run_cmd(c, 12).await {
            if out_path.exists() { return Ok(()); }
        }
    }

    try_cmd!("import", ["-window", "root", &path_str]);

    if probe("ffmpeg").await {
        let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".to_string());
        let mut c = Command::new("ffmpeg");
        c.args(["-y", "-hide_banner", "-loglevel", "error",
                "-f", "x11grab", "-video_size", "1920x1080", "-i", &display,
                "-vframes", "1", &path_str]);
        if let Ok(_) = run_cmd(c, 12).await {
            if out_path.exists() { return Ok(()); }
        }

        if std::env::var("WAYLAND_DISPLAY").is_ok() {
            let mut c2 = Command::new("sh");
            c2.arg("-c").arg(format!(
                "grim '{}' 2>/dev/null || wayshot -f '{}' 2>/dev/null",
                path_str, path_str
            ));
            let _ = run_cmd(c2, 10).await;
            if out_path.exists() { return Ok(()); }
        }
    }

    if probe("xwd").await && probe("convert").await {
        let xwd = out_path.with_extension("xwd");
        let xwd_str = xwd.to_string_lossy().to_string();
        let mut c = Command::new("sh");
        c.arg("-c").arg(format!("xwd -root -silent > '{}' && convert '{}' '{}'", xwd_str, xwd_str, path_str));
        if let Ok(_) = run_cmd(c, 12).await {
            let _ = tokio::fs::remove_file(&xwd).await;
            if out_path.exists() { return Ok(()); }
        }
    }

    Err(format!(
        "No working screenshot tool on Linux. Tried. Errors: {}. Install scrot/gnome-screenshot/spectacle/flameshot/ffmpeg.",
        if errors.is_empty() { "none available".to_string() } else { errors.join("; ") }
    ))
}

async fn capture_webcam_windows(out_path: &PathBuf, device_hint: Option<&str>) -> Result<(), String> {
    let path_escaped = out_path.to_string_lossy().replace('"', "\\\"");
    let mut errors: Vec<String> = Vec::new();

    if probe("ffmpeg").await {
        let video_devices = ffmpeg_dshow_devices(true).await;
        let candidates: Vec<String> = if let Some(hint) = device_hint {
            vec![hint.to_string()]
        } else if !video_devices.is_empty() {
            video_devices
        } else {
            vec![]
        };

        for device_name in &candidates {
            let mut cmd = Command::new("ffmpeg");
            cmd.args([
                "-y", "-hide_banner", "-loglevel", "error",
                "-f", "dshow",
                "-i", &format!("video={}", device_name),
                "-vframes", "1", "-q:v", "2",
                &path_escaped,
            ]);
            if let Ok(out) = run_cmd(cmd, 25).await {
                if out.status.success() && out_path.exists() { return Ok(()); }
                errors.push(format!("ffmpeg dshow {}: {}", device_name, String::from_utf8_lossy(&out.stderr).trim()));
            }
        }
        if candidates.is_empty() {
            errors.push("ffmpeg found but no DirectShow video devices detected".to_string());
        }
    }

    for python in &["python", "python3"] {
        if probe(python).await {
            let script = format!(
                "import cv2,sys; cap=cv2.VideoCapture({}); ret,f=cap.read(); \
                 r=cv2.imwrite(sys.argv[1],f) if ret else False; cap.release(); \
                 print('ok' if r else 'fail')",
                device_hint.and_then(|d| d.parse::<i32>().ok()).unwrap_or(0)
            );
            let mut cmd = Command::new(python);
            cmd.args(["-c", &script, &path_escaped]);
            if let Ok(out) = run_cmd(cmd, 20).await {
                if out_path.exists() { return Ok(()); }
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                if !stderr.contains("ModuleNotFoundError") {
                    errors.push(format!("{} cv2: {}", python, stderr));
                }
            }
        }
    }

    if probe("vlc").await {
        let path_str = out_path.to_string_lossy().to_string();
        let snapshot_dir = out_path.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|| ".".to_string());
        let base = out_path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "frame".to_string());
        let device_idx = device_hint.and_then(|d| d.parse::<u32>().ok()).unwrap_or(0);
        let mut cmd = Command::new("vlc");
        cmd.args([
            &format!("dshow:// :dshow-vdev={}", device_idx),
            "--no-audio", "--video-filter=scene", "--scene-format=jpg",
            "--scene-ratio=1", &format!("--scene-prefix={}", base),
            &format!("--scene-path={}", snapshot_dir),
            "--play-and-exit", "--rate=1", "--stop-time=1", "--intf=dummy",
        ]);
        if let Ok(_) = run_cmd(cmd, 15).await {
            let candidate = std::path::PathBuf::from(&snapshot_dir).join(format!("{}00001.jpg", base));
            if candidate.exists() {
                let _ = tokio::fs::rename(&candidate, &path_str).await;
                if out_path.exists() { return Ok(()); }
            }
        }
        errors.push("vlc: did not produce a snapshot".to_string());
    }

    let winrt_script = format!(
        "$p='{path}'
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Runtime.WindowsRuntime
$null=[Windows.Media.Capture.MediaCapture,Windows.Media.Capture,ContentType=WindowsRuntime]
$null=[Windows.Storage.StorageFile,Windows.Storage,ContentType=WindowsRuntime]
$null=[Windows.Media.MediaProperties.ImageEncodingProperties,Windows.Media.MediaProperties,ContentType=WindowsRuntime]
function Await($t){{
    $a=$t.GetType().GetMethod('GetAwaiter').Invoke($t,@())
    while(-not $a.IsCompleted){{Start-Sleep -Milliseconds 50}}
    $m=$a.GetType().GetMethod('GetResult')
    if($m){{return $m.Invoke($a,@())}}
}}
$cap=New-Object Windows.Media.Capture.MediaCapture
Await($cap.InitializeAsync())|Out-Null
$props=[Windows.Media.MediaProperties.ImageEncodingProperties]::CreateJpeg()
$sf=Await([Windows.Storage.StorageFolder]::GetFolderFromPathAsync([System.IO.Path]::GetDirectoryName($p)))
$file=Await($sf.CreateFileAsync([System.IO.Path]::GetFileName($p),[Windows.Storage.CreationCollisionOption]::ReplaceExisting))
Await($cap.CapturePhotoToStorageFileAsync($props,$file))|Out-Null
$cap.Dispose()
Write-Output 'ok'",
        path = out_path.to_string_lossy().replace('\'', "''")
    );

    match powershell_run(&winrt_script, 30).await {
        Ok(_) if out_path.exists() => return Ok(()),
        Ok(o) => errors.push(format!("WinRT: {}", o)),
        Err(e) => errors.push(format!("WinRT: {}", e)),
    }

    Err(format!(
        "Webcam capture failed on all backends. Tried: {}. \
         Fix: install ffmpeg (winget install Gyan.FFmpeg) or install Python with opencv-python (pip install opencv-python).",
        errors.join(" | ")
    ))
}

async fn capture_webcam_macos(out_path: &PathBuf, device_hint: Option<&str>) -> Result<(), String> {
    let path_str = out_path.to_string_lossy().to_string();

    if probe("imagesnap").await {
        let mut cmd = Command::new("imagesnap");
        if let Some(dev) = device_hint { cmd.args(["-d", dev]); }
        cmd.arg(&path_str);
        let out = run_cmd(cmd, 20).await?;
        if out_path.exists() { return Ok(()); }
        let e = String::from_utf8_lossy(&out.stderr).to_string();
        if !e.is_empty() { return Err(format!("imagesnap: {}", e)); }
    }

    if probe("ffmpeg").await {
        for idx in &["0", "1", "default"] {
            let mut cmd = Command::new("ffmpeg");
            cmd.args([
                "-y", "-hide_banner", "-loglevel", "error",
                "-f", "avfoundation",
                "-i", &format!("{}:none", idx),
                "-vframes", "1", "-q:v", "2",
                &path_str,
            ]);
            if let Ok(_) = run_cmd(cmd, 20).await {
                if out_path.exists() { return Ok(()); }
            }
        }
        return Err("ffmpeg avfoundation: all device indices failed. Check: ffmpeg -f avfoundation -list_devices true -i \"\"".to_string());
    }

    Err("Webcam on macOS needs imagesnap (brew install imagesnap) or ffmpeg (brew install ffmpeg)".to_string())
}

async fn capture_webcam_linux(out_path: &PathBuf, device_hint: Option<&str>) -> Result<(), String> {
    let path_str = out_path.to_string_lossy().to_string();

    let device = if let Some(d) = device_hint {
        d.to_string()
    } else {
        find_linux_video_device().await.unwrap_or_else(|| "/dev/video0".to_string())
    };

    if probe("ffmpeg").await {
        let mut cmd = Command::new("ffmpeg");
        cmd.args([
            "-y", "-hide_banner", "-loglevel", "error",
            "-f", "v4l2", "-i", &device,
            "-vframes", "1", "-q:v", "2",
            &path_str,
        ]);
        let out = run_cmd(cmd, 20).await?;
        if out_path.exists() { return Ok(()); }
        return Err(format!("ffmpeg v4l2 on {}: {}", device, String::from_utf8_lossy(&out.stderr)));
    }

    if probe("streamer").await {
        let mut cmd = Command::new("streamer");
        cmd.args(["-c", &device, "-b", "16", "-o", &path_str]);
        if let Ok(_) = run_cmd(cmd, 20).await {
            if out_path.exists() { return Ok(()); }
        }
    }

    Err(format!("Webcam on Linux needs ffmpeg with v4l2. Install: sudo apt install ffmpeg. Device: {}", device))
}

async fn find_linux_video_device() -> Option<String> {
    let mut dir = tokio::fs::read_dir("/dev").await.ok()?;
    let mut devices = Vec::new();
    while let Ok(Some(entry)) = dir.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("video") {
            if let Ok(n) = name["video".len()..].parse::<u32>() {
                devices.push((n, format!("/dev/{}", name)));
            }
        }
    }
    devices.sort_by_key(|(n, _)| *n);
    devices.into_iter().next().map(|(_, p)| p)
}

async fn record_audio_windows(out_path: &PathBuf, duration_secs: u64, device_hint: Option<&str>) -> Result<(), String> {
    let wav_path = out_path.with_extension("wav");
    let duration_ms = duration_secs * 1000;
    let mut errors: Vec<String> = Vec::new();

    let mci_script = format!(
        "Add-Type -TypeDefinition @\"
using System;
using System.Threading;
using System.Runtime.InteropServices;
using System.Text;
public class MciRecorder {{
    [DllImport(\"winmm.dll\", CharSet=CharSet.Auto)]
    static extern int mciSendString(string cmd, StringBuilder ret, int retLen, IntPtr hwnd);
    public static string Record(string path, int ms) {{
        var sb = new StringBuilder(256);
        mciSendString(\"open new type waveaudio alias rec\", sb, 256, IntPtr.Zero);
        mciSendString(\"set rec bitspersample 16 channels 2 samplespersec 44100\", sb, 256, IntPtr.Zero);
        mciSendString(\"record rec\", sb, 256, IntPtr.Zero);
        Thread.Sleep(ms);
        mciSendString(\"stop rec\", sb, 256, IntPtr.Zero);
        int r = mciSendString(\"save rec \\\"\" + path + \"\\\"\", sb, 256, IntPtr.Zero);
        mciSendString(\"close rec\", sb, 256, IntPtr.Zero);
        return r == 0 ? \"ok\" : \"err:\" + r;
    }}
}}
\"@
$p = [System.Text.Encoding]::UTF8.GetString([System.Convert]::FromBase64String('{wav_b64}'))
$result = [MciRecorder]::Record($p, {ms})
Write-Output $result",
        wav_b64 = base64_encode(wav_path.to_string_lossy().as_bytes()),
        ms = duration_ms
    );

    let mci_result = powershell_run(&mci_script, duration_secs + 30).await;
    let mci_ok = mci_result.as_ref().map(|s| s.contains("ok")).unwrap_or(false) && wav_path.exists();

    if mci_ok {
        if probe("ffmpeg").await && out_path.extension().and_then(|e| e.to_str()) != Some("wav") {
            let fp = out_path.to_string_lossy().replace('"', "\\\"");
            let wp = wav_path.to_string_lossy().replace('"', "\\\"");
            let mut cmd = Command::new("ffmpeg");
            cmd.args(["-y", "-hide_banner", "-loglevel", "error", "-i", &wp, &fp]);
            if run_cmd(cmd, 30).await.map(|o| o.status.success()).unwrap_or(false) && out_path.exists() {
                let _ = tokio::fs::remove_file(&wav_path).await;
                return Ok(());
            }
        }
        if wav_path != *out_path {
            let _ = tokio::fs::rename(&wav_path, out_path).await;
        }
        if out_path.exists() || wav_path.exists() { return Ok(()); }
    } else {
        errors.push(format!("MCI winmm.dll: {}", mci_result.as_ref().err().map(|s| s.as_str()).unwrap_or("file not created")));
    }

    if probe("ffmpeg").await {
        let audio_devices = ffmpeg_dshow_devices(false).await;
        let duration_str = duration_secs.to_string();
        let path_escaped = out_path.to_string_lossy().replace('"', "\\\"");

        let candidates: Vec<String> = if let Some(hint) = device_hint {
            vec![format!("audio={}", hint)]
        } else if !audio_devices.is_empty() {
            audio_devices.iter().map(|d| format!("audio={}", d)).collect()
        } else {
            vec!["audio=default".to_string()]
        };

        for audio_input in &candidates {
            let mut cmd = Command::new("ffmpeg");
            cmd.args([
                "-y", "-hide_banner", "-loglevel", "error",
                "-f", "dshow", "-i", audio_input,
                "-t", &duration_str, &path_escaped,
            ]);
            if let Ok(out) = run_cmd(cmd, duration_secs + 20).await {
                if out.status.success() && out_path.exists() { return Ok(()); }
                errors.push(format!("ffmpeg dshow {}: {}", audio_input, String::from_utf8_lossy(&out.stderr).trim()));
            }
        }

        let mut cmd2 = Command::new("ffmpeg");
        cmd2.args([
            "-y", "-hide_banner", "-loglevel", "error",
            "-f", "wasapi", "-i", "default",
            "-t", &duration_str, &path_escaped,
        ]);
        match run_cmd(cmd2, duration_secs + 20).await {
            Ok(out) if out.status.success() && out_path.exists() => return Ok(()),
            Ok(out) => errors.push(format!("ffmpeg wasapi: {}", String::from_utf8_lossy(&out.stderr).trim())),
            Err(e) => errors.push(format!("ffmpeg wasapi: {}", e)),
        }
    }

    for python in &["python", "python3"] {
        if probe(python).await {
            let duration_f = duration_secs as f64;
            let path_escaped_py = out_path.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
            let script = format!(
                "import sounddevice as sd,scipy.io.wavfile as wf,numpy as np; \
                 d=sd.rec(int({dur}*44100),samplerate=44100,channels=1,dtype='int16'); \
                 sd.wait(); wf.write('{path}',44100,d); print('ok')",
                dur = duration_f,
                path = path_escaped_py
            );
            let mut cmd = Command::new(python);
            cmd.args(["-c", &script]);
            if let Ok(out) = run_cmd(cmd, duration_secs + 30).await {
                if out_path.exists() { return Ok(()); }
                let se = String::from_utf8_lossy(&out.stderr).trim().to_string();
                if !se.contains("ModuleNotFoundError") {
                    errors.push(format!("{} sounddevice: {}", python, se));
                }
            }
        }
    }

    Err(format!(
        "Audio recording failed on all backends. Errors: {}. \
         Fix: install ffmpeg (winget install Gyan.FFmpeg) or Python with sounddevice+scipy (pip install sounddevice scipy).",
        errors.join(" | ")
    ))
}

fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = if chunk.len() > 1 { chunk[1] as usize } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as usize } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(n >> 18) & 63] as char);
        out.push(TABLE[(n >> 12) & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n & 63] as char } else { '=' });
    }
    out
}

async fn record_audio_macos(out_path: &PathBuf, duration_secs: u64, _device_hint: Option<&str>) -> Result<(), String> {
    let path_str = out_path.to_string_lossy().to_string();
    let duration_str = duration_secs.to_string();

    if probe("ffmpeg").await {
        for audio_input in &[":0", ":default", "none:0", "none:default"] {
            let mut cmd = Command::new("ffmpeg");
            cmd.args([
                "-y", "-hide_banner", "-loglevel", "error",
                "-f", "avfoundation", "-i", audio_input,
                "-t", &duration_str, &path_str,
            ]);
            if let Ok(_) = run_cmd(cmd, duration_secs + 20).await {
                if out_path.exists() { return Ok(()); }
            }
        }
    }

    if probe("rec").await {
        let mut cmd = Command::new("rec");
        cmd.args(["-t", "mp3", &path_str, "trim", "0", &duration_str]);
        if let Ok(out) = run_cmd(cmd, duration_secs + 20).await {
            if out.status.success() && out_path.exists() { return Ok(()); }
        }
    }

    Err("Audio on macOS needs ffmpeg (brew install ffmpeg) or sox (brew install sox)".to_string())
}

async fn record_audio_linux(out_path: &PathBuf, duration_secs: u64, device_hint: Option<&str>) -> Result<(), String> {
    let path_str = out_path.to_string_lossy().to_string();
    let duration_str = duration_secs.to_string();
    let mut errors = Vec::<String>::new();

    if probe("ffmpeg").await {
        for (label, fmt, iarg) in &[
            ("pulse", "pulse", "default"),
            ("pipewire", "pipewire", "default"),
            ("alsa-default", "alsa", "default"),
            ("alsa-hw0", "alsa", "hw:0,0"),
        ] {
            let input = device_hint.unwrap_or(iarg);
            let mut cmd = Command::new("ffmpeg");
            cmd.args(["-y", "-hide_banner", "-loglevel", "error",
                      "-f", fmt, "-i", input,
                      "-t", &duration_str, &path_str]);
            match run_cmd(cmd, duration_secs + 20).await {
                Ok(out) if out.status.success() && out_path.exists() => return Ok(()),
                Ok(out) => errors.push(format!("{}: {}", label, String::from_utf8_lossy(&out.stderr).trim())),
                Err(e) => errors.push(format!("{}: {}", label, e)),
            }
        }
    }

    if probe("arecord").await {
        let device = device_hint.unwrap_or("default");
        let wav_path = out_path.with_extension("wav");
        let wav_str = wav_path.to_string_lossy().to_string();
        let mut cmd = Command::new("arecord");
        cmd.args(["-D", device, "-d", &duration_str, "-f", "cd", &wav_str]);
        match run_cmd(cmd, duration_secs + 20).await {
            Ok(out) if out.status.success() && wav_path.exists() => {
                if probe("ffmpeg").await && out_path.extension().and_then(|e| e.to_str()) != Some("wav") {
                    let mut conv = Command::new("ffmpeg");
                    conv.args(["-y", "-hide_banner", "-loglevel", "error",
                               "-i", &wav_str, &path_str]);
                    let _ = run_cmd(conv, 30).await;
                    let _ = tokio::fs::remove_file(&wav_path).await;
                    if out_path.exists() { return Ok(()); }
                }
                if tokio::fs::rename(&wav_path, out_path).await.is_ok() { return Ok(()); }
                return Ok(());
            }
            Ok(out) => errors.push(format!("arecord: {}", String::from_utf8_lossy(&out.stderr).trim())),
            Err(e) => errors.push(format!("arecord: {}", e)),
        }
    }

    if probe("parecord").await {
        let wav_path = out_path.with_extension("wav");
        let wav_str = wav_path.to_string_lossy().to_string();
        if let Ok(mut child) = tokio::process::Command::new("parecord")
            .args(["--file-format=wav", &wav_str])
            .spawn()
        {
            tokio::time::sleep(Duration::from_secs(duration_secs)).await;
            let _ = child.kill().await;
            let _ = child.wait().await;
            if wav_path.exists() {
                if probe("ffmpeg").await && out_path.extension().and_then(|e| e.to_str()) != Some("wav") {
                    let mut conv = Command::new("ffmpeg");
                    conv.args(["-y", "-hide_banner", "-loglevel", "error",
                               "-i", &wav_str, &path_str]);
                    let _ = run_cmd(conv, 30).await;
                    let _ = tokio::fs::remove_file(&wav_path).await;
                    if out_path.exists() { return Ok(()); }
                }
                if tokio::fs::rename(&wav_path, out_path).await.is_ok() { return Ok(()); }
            }
        }
    }

    Err(format!(
        "Audio recording failed on Linux. Errors: {}. Install: sudo apt install ffmpeg",
        if errors.is_empty() { "no audio tool found".to_string() } else { errors.join("; ") }
    ))
}

async fn record_screen_windows(out_path: &PathBuf, duration_secs: u64) -> Result<(), String> {
    let duration_str = duration_secs.to_string();
    let path_escaped = out_path.to_string_lossy().replace('"', "\\\"");

    if probe("ffmpeg").await {
        let mut cmd = Command::new("ffmpeg");
        cmd.args([
            "-y", "-hide_banner", "-loglevel", "error",
            "-f", "gdigrab",
            "-framerate", "15",
            "-i", "desktop",
            "-t", &duration_str,
            "-c:v", "libx264",
            "-preset", "ultrafast",
            "-pix_fmt", "yuv420p",
            &path_escaped,
        ]);
        let out = run_cmd(cmd, duration_secs + 30).await?;
        if out.status.success() && out_path.exists() { return Ok(()); }
        return Err(format!(
            "ffmpeg gdigrab screen recording failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }

    let tmpdir_script = r#"
$tmpdir = [System.IO.Path]::Combine([System.IO.Path]::GetTempPath(), 'panther_frames_' + [System.Guid]::NewGuid().ToString('N'))
[System.IO.Directory]::CreateDirectory($tmpdir) | Out-Null
Write-Output $tmpdir
"#;
    let tmpdir = match powershell_run(tmpdir_script, 10).await {
        Ok(s) => s.trim().to_string(),
        Err(e) => return Err(format!("Failed to create temp dir: {}", e)),
    };

    let fps = 10u64;
    let total_frames = duration_secs * fps;
    let interval_ms = 1000u64 / fps;

    let capture_script = format!(
        "Add-Type -AssemblyName System.Windows.Forms,System.Drawing\n\
         $w=[System.Windows.Forms.Screen]::PrimaryScreen.Bounds.Width\n\
         $h=[System.Windows.Forms.Screen]::PrimaryScreen.Bounds.Height\n\
         $fmt=[System.Drawing.Imaging.ImageFormat]::Png\n\
         for($i=0; $i -lt {total}; $i++) {{\n\
             $bmp=New-Object System.Drawing.Bitmap($w,$h)\n\
             $g=[System.Drawing.Graphics]::FromImage($bmp)\n\
             $g.CopyFromScreen(0,0,0,0,[System.Drawing.Size]::new($w,$h))\n\
             $fname='{tmpdir}\\' + $i.ToString('D6') + '.png'\n\
             $bmp.Save($fname,$fmt)\n\
             $g.Dispose(); $bmp.Dispose()\n\
             Start-Sleep -Milliseconds {interval}\n\
         }}\n\
         Write-Output 'done'",
        total = total_frames,
        tmpdir = tmpdir.replace('\'', "''"),
        interval = interval_ms
    );

    let capture_result = powershell_run(&capture_script, duration_secs + 60).await;
    let capture_ok = capture_result.as_ref().map(|s| s.contains("done")).unwrap_or(false);

    if capture_ok {
        let tmpdir_esc = tmpdir.replace('\"', "\\\"");
        let tmpdir_ps = tmpdir.replace('\'', "''");

        if probe("ffmpeg").await {
            let glob_pattern = format!("{}\\%06d.png", tmpdir_esc);
            let mut cmd = Command::new("ffmpeg");
            cmd.args([
                "-y", "-hide_banner", "-loglevel", "error",
                "-framerate", &fps.to_string(),
                "-i", &glob_pattern,
                "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p",
                &path_escaped,
            ]);
            let ffmpeg_out = run_cmd(cmd, 120).await;
            let _ = tokio::process::Command::new("cmd")
                .args(["/C", &format!("rmdir /s /q \"{}\"", tmpdir)])
                .output().await;
            match ffmpeg_out {
                Ok(o) if o.status.success() && out_path.exists() => return Ok(()),
                Ok(o) => return Err(format!("ffmpeg frame assembly failed: {}", String::from_utf8_lossy(&o.stderr))),
                Err(e) => return Err(format!("ffmpeg launch failed: {}", e)),
            }
        }

        for python in &["python", "python3"] {
            if probe(python).await {
                let path_py = out_path.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
                let script = format!(
                    "import cv2,glob,os; frames=sorted(glob.glob('{tmpdir}\\\\*.png')); \
                     h,w=cv2.imread(frames[0]).shape[:2] if frames else (0,0); \
                     out=cv2.VideoWriter('{path}',cv2.VideoWriter_fourcc(*'mp4v'),{fps},( w,h)); \
                     [out.write(cv2.imread(f)) for f in frames]; out.release(); \
                     [os.remove(f) for f in frames]; print('ok')",
                    tmpdir = tmpdir.replace('\\', "\\\\"),
                    path = path_py,
                    fps = fps
                );
                let mut cmd = Command::new(python);
                cmd.args(["-c", &script]);
                if let Ok(out) = run_cmd(cmd, 120).await {
                    if out_path.exists() {
                        let _ = tokio::process::Command::new("cmd")
                            .args(["/C", &format!("rmdir /s /q \"{}\"", tmpdir)])
                            .output().await;
                        return Ok(());
                    }
                    let se = String::from_utf8_lossy(&out.stderr).trim().to_string();
                    if se.contains("ModuleNotFoundError") {
                        break;
                    }
                }
            }
        }

        let gif_path = out_path.with_extension("gif");
        let gif_ps = gif_path.to_string_lossy().replace('\'', "''");
        let gif_script = format!(
            "Add-Type -AssemblyName System.Drawing\n\
             $frames = Get-ChildItem -Path '{tmpdir}' -Filter '*.png' | Sort-Object Name\n\
             $first = [System.Drawing.Image]::FromFile($frames[0].FullName)\n\
             $enc = [System.Drawing.Imaging.ImageCodecInfo]::GetImageEncoders() | Where-Object {{ $_.FormatDescription -eq 'GIF' }}\n\
             $ep = New-Object System.Drawing.Imaging.EncoderParameters(1)\n\
             $ep.Param[0] = New-Object System.Drawing.Imaging.EncoderParameter([System.Drawing.Imaging.Encoder]::SaveFlag, 87L)\n\
             $first.Save('{gif}', $enc, $ep)\n\
             $ep.Param[0] = New-Object System.Drawing.Imaging.EncoderParameter([System.Drawing.Imaging.Encoder]::SaveFlag, 102L)\n\
             foreach ($f in $frames[1..($frames.Count-2)]) {{\n\
                 $img = [System.Drawing.Image]::FromFile($f.FullName)\n\
                 $first.SaveAdd($img, $ep)\n\
                 $img.Dispose()\n\
             }}\n\
             $ep.Param[0] = New-Object System.Drawing.Imaging.EncoderParameter([System.Drawing.Imaging.Encoder]::SaveFlag, 88L)\n\
             $img = [System.Drawing.Image]::FromFile($frames[-1].FullName)\n\
             $first.SaveAdd($img, $ep)\n\
             $img.Dispose(); $first.Dispose()\n\
             Write-Output 'done'",
            tmpdir = tmpdir_ps,
            gif = gif_ps
        );

        match powershell_run(&gif_script, duration_secs + 60).await {
            Ok(_) if gif_path.exists() => {
                let _ = tokio::process::Command::new("cmd")
                    .args(["/C", &format!("rmdir /s /q \"{}\"", tmpdir)])
                    .output().await;
                let _ = tokio::fs::rename(&gif_path, out_path).await;
                if out_path.exists() { return Ok(()); }
            }
            _ => {}
        }

        let _ = tokio::process::Command::new("cmd")
            .args(["/C", &format!("rmdir /s /q \"{}\"", tmpdir)])
            .output().await;

        return Err(
            "Frames captured but could not encode video: ffmpeg missing, Python cv2 unavailable, and GIF encoding failed. \
             Install ffmpeg (winget install Gyan.FFmpeg) or Python with opencv-python (pip install opencv-python).".to_string()
        );
    }

    let _ = tokio::process::Command::new("cmd")
        .args(["/C", &format!("rmdir /s /q \"{}\"", tmpdir)])
        .output().await;

    Err(format!(
        "Screen recording failed: ffmpeg not found AND PowerShell frame capture failed: {}. Install ffmpeg: winget install Gyan.FFmpeg",
        capture_result.err().unwrap_or_else(|| "no output".to_string())
    ))
}

async fn record_screen_macos(out_path: &PathBuf, duration_secs: u64) -> Result<(), String> {
    let path_str = out_path.to_string_lossy().to_string();
    let duration_str = duration_secs.to_string();

    if probe("ffmpeg").await {
        let display = ":0".to_string();
        let mut cmd = Command::new("ffmpeg");
        cmd.args([
            "-y", "-hide_banner", "-loglevel", "error",
            "-f", "avfoundation",
            "-i", &format!("{}:none", display),
            "-t", &duration_str,
            "-c:v", "libx264", "-pix_fmt", "yuv420p",
            &path_str,
        ]);
        let out = run_cmd(cmd, duration_secs + 30).await?;
        if out.status.success() && out_path.exists() { return Ok(()); }

        let mut cmd2 = Command::new("ffmpeg");
        cmd2.args([
            "-y", "-hide_banner", "-loglevel", "error",
            "-f", "avfoundation",
            "-i", "Capture screen 0:none",
            "-t", &duration_str,
            "-c:v", "libx264", "-pix_fmt", "yuv420p",
            &path_str,
        ]);
        let out2 = run_cmd(cmd2, duration_secs + 30).await?;
        if out2.status.success() && out_path.exists() { return Ok(()); }
        return Err(format!("ffmpeg avfoundation screen: {}", String::from_utf8_lossy(&out2.stderr)));
    }

    Err("Screen recording on macOS needs ffmpeg (brew install ffmpeg)".to_string())
}

async fn record_screen_linux(out_path: &PathBuf, duration_secs: u64) -> Result<(), String> {
    let path_str = out_path.to_string_lossy().to_string();
    let duration_str = duration_secs.to_string();

    if probe("ffmpeg").await {
        let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".to_string());

        let mut cmd = Command::new("ffmpeg");
        cmd.args([
            "-y", "-hide_banner", "-loglevel", "error",
            "-f", "x11grab", "-video_size", "1920x1080",
            "-i", &display,
            "-t", &duration_str,
            "-c:v", "libx264", "-pix_fmt", "yuv420p",
            &path_str,
        ]);
        let out = run_cmd(cmd, duration_secs + 30).await?;
        if out.status.success() && out_path.exists() { return Ok(()); }

        if std::env::var("WAYLAND_DISPLAY").is_ok() {
            for tool in &["wf-recorder", "wl-screenrec"] {
                if probe(tool).await {
                    let mut cmd2 = Command::new(tool);
                    cmd2.args(["-f", &path_str]);
                    if let Ok(mut child) = cmd2.spawn() {
                        tokio::time::sleep(Duration::from_secs(duration_secs)).await;
                        let _ = child.kill().await;
                        let _ = child.wait().await;
                        if out_path.exists() { return Ok(()); }
                    }
                }
            }
        }

        return Err(format!("ffmpeg x11grab failed: {}", String::from_utf8_lossy(&out.stderr)));
    }

    if probe("recordmydesktop").await {
        let out_ogv = out_path.with_extension("ogv");
        let ogv_str = out_ogv.to_string_lossy().to_string();
        if let Ok(mut child) = tokio::process::Command::new("recordmydesktop")
            .args(["--no-sound", "-o", &ogv_str])
            .spawn()
        {
            tokio::time::sleep(Duration::from_secs(duration_secs)).await;
            let _ = child.kill().await;
            let _ = child.wait().await;
            if out_ogv.exists() {
                if probe("ffmpeg").await {
                    let mut conv = Command::new("ffmpeg");
                    conv.args(["-y", "-hide_banner", "-loglevel", "error",
                               "-i", &ogv_str, &path_str]);
                    let _ = run_cmd(conv, 60).await;
                    let _ = tokio::fs::remove_file(&out_ogv).await;
                    if out_path.exists() { return Ok(()); }
                }
                if tokio::fs::rename(&out_ogv, out_path).await.is_ok() { return Ok(()); }
            }
        }
    }

    Err("Screen recording on Linux needs ffmpeg (sudo apt install ffmpeg)".to_string())
}

async fn record_screen(out_path: &PathBuf, duration_secs: u64) -> Result<(), String> {
    if cfg!(target_os = "windows") { record_screen_windows(out_path, duration_secs).await }
    else if cfg!(target_os = "macos") { record_screen_macos(out_path, duration_secs).await }
    else { record_screen_linux(out_path, duration_secs).await }
}

async fn capture_screenshot(out_path: &PathBuf) -> Result<(), String> {
    if cfg!(target_os = "windows") { capture_screenshot_windows(out_path).await }
    else if cfg!(target_os = "macos") { capture_screenshot_macos(out_path).await }
    else { capture_screenshot_linux(out_path).await }
}

async fn capture_webcam(out_path: &PathBuf, device_hint: Option<&str>) -> Result<(), String> {
    if cfg!(target_os = "windows") { capture_webcam_windows(out_path, device_hint).await }
    else if cfg!(target_os = "macos") { capture_webcam_macos(out_path, device_hint).await }
    else { capture_webcam_linux(out_path, device_hint).await }
}

async fn record_audio(out_path: &PathBuf, duration_secs: u64, device_hint: Option<&str>) -> Result<(), String> {
    if cfg!(target_os = "windows") { record_audio_windows(out_path, duration_secs, device_hint).await }
    else if cfg!(target_os = "macos") { record_audio_macos(out_path, duration_secs, device_hint).await }
    else { record_audio_linux(out_path, duration_secs, device_hint).await }
}

impl Tool for CaptureMediaTool {
    fn name(&self) -> &str { "capture_media" }

    fn description(&self) -> &str {
        "Capture media from this machine: screenshot (full screen), webcam photo/selfie, or microphone audio recording. \
         Windows: uses native PowerShell/MCI (no external tools needed) with ffmpeg as enhanced fallback. \
         macOS: uses screencapture/imagesnap/avfoundation. \
         Linux: uses v4l2/pulse/pipewire/alsa. \
         Always appends [PANTHER_FILE:path] so the file is automatically sent to chat."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "type": {
                    "type": "string",
                    "enum": ["screenshot", "webcam", "audio", "screen_record"],
                    "description": "What to capture: 'screenshot' = full screen image, 'webcam' = camera/selfie photo, 'audio' = microphone recording, 'screen_record' = video recording of screen"
                },
                "duration_secs": {
                    "type": "integer",
                    "description": "Duration in seconds for audio recording (default: 10, max: 300, min: 1)"
                },
                "device": {
                    "type": "string",
                    "description": "Optional device override. Windows: dshow device name e.g. 'HD Pro Webcam C920'. Linux: path e.g. '/dev/video1'. macOS: avfoundation index e.g. '1'. Leave empty to auto-detect."
                },
                "filename": {
                    "type": "string",
                    "description": "Optional custom output filename without extension (timestamp used by default)"
                }
            },
            "required": ["type"]
        })
    }

    fn execute<'a>(&'a self, args: serde_json::Value) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
        let temp_dir = self.temp_dir.clone();
        let turn_cache = Arc::clone(&self.turn_cache);
        Box::pin(async move {
            let capture_type = match args.get("type").and_then(|v| v.as_str()) {
                Some(t) => t.to_string(),
                None => return "Error: missing 'type' argument".to_string(),
            };
            let device = args.get("device").and_then(|v| v.as_str()).map(|s| s.to_string());
            let custom_name = args.get("filename").and_then(|v| v.as_str()).map(|s| s.to_string());
            let duration_secs = args.get("duration_secs")
                .and_then(|v| v.as_u64())
                .unwrap_or(10)
                .clamp(1, 300);

            let cache_key = format!("{}:{}", capture_type, device.as_deref().unwrap_or(""));
            {
                let cache = turn_cache.lock().await;
                if let Some(cached_path) = cache.get(&cache_key) {
                    if cached_path.exists() {
                        return format!("[PANTHER_FILE:{}]", cached_path.display());
                    }
                }
            }

            if let Err(e) = tokio::fs::create_dir_all(&temp_dir).await {
                return format!("Error creating temp dir: {}", e);
            }

            let ts = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();

            macro_rules! cache_and_return {
                ($label:expr, $path:expr) => {{
                    turn_cache.lock().await.insert(cache_key.clone(), $path.clone());
                    format!("{} [PANTHER_FILE:{}]", $label, $path.display())
                }};
            }

            match capture_type.as_str() {
                "screenshot" => {
                    let base = custom_name.unwrap_or_else(|| format!("screenshot_{}", ts));
                    let out_path = temp_dir.join(format!("{}.png", base));
                    match capture_screenshot(&out_path).await {
                        Ok(()) if out_path.exists() => cache_and_return!("Screenshot captured.", out_path),
                        Ok(()) => "Error: screenshot command succeeded but file was not written".to_string(),
                        Err(e) => format!("Screenshot failed: {}", e),
                    }
                }
                "webcam" => {
                    let base = custom_name.unwrap_or_else(|| format!("webcam_{}", ts));
                    let out_path = temp_dir.join(format!("{}.jpg", base));
                    match capture_webcam(&out_path, device.as_deref()).await {
                        Ok(()) if out_path.exists() => cache_and_return!("Webcam photo captured.", out_path),
                        Ok(()) => "Error: webcam command succeeded but file was not written".to_string(),
                        Err(e) => format!("Webcam capture failed: {}", e),
                    }
                }
                "audio" => {
                    let base = custom_name.unwrap_or_else(|| format!("audio_{}", ts));
                    let has_ffmpeg = probe("ffmpeg").await;
                    let preferred_ext = if has_ffmpeg { "mp3" } else { "wav" };
                    let out_path = temp_dir.join(format!("{}.{}", base, preferred_ext));
                    match record_audio(&out_path, duration_secs, device.as_deref()).await {
                        Ok(()) => {
                            let actual = if out_path.exists() {
                                out_path
                            } else {
                                let wav = temp_dir.join(format!("{}.wav", base));
                                let mp3 = temp_dir.join(format!("{}.mp3", base));
                                if wav.exists() {
                                    wav
                                } else if mp3.exists() {
                                    mp3
                                } else {
                                    return "Error: audio recording succeeded but no output file was found on disk".to_string();
                                }
                            };
                            cache_and_return!(format!("Audio recorded ({}s).", duration_secs), actual)
                        }
                        Err(e) => format!("Audio recording failed: {}", e),
                    }
                }
                "screen_record" => {
                    let base = custom_name.unwrap_or_else(|| format!("screen_{}", ts));
                    let out_path = temp_dir.join(format!("{}.mp4", base));
                    match record_screen(&out_path, duration_secs).await {
                        Ok(()) if out_path.exists() => cache_and_return!(format!("Screen recording done ({}s).", duration_secs), out_path),
                        Ok(()) => "Error: screen recording succeeded but file was not written".to_string(),
                        Err(e) => format!("Screen recording failed: {}", e),
                    }
                }
                other => format!("Unknown type '{}'. Use: screenshot, webcam, audio, screen_record", other),
            }
        })
    }
}
