use std::time::Duration;

use shared::errors::{PantherError, PantherResult};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::timeout;
use uuid::Uuid;

const TIMEOUT_SECS: u64 = 30;

async fn run_subprocess_path(cmd: &mut Command, input: &str, env_vars: &[(&str, &str)]) -> PantherResult<String> {
    cmd.env("PANTHER_INPUT", input);
    for (key, val) in env_vars {
        cmd.env(key, val);
    }
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| PantherError::ExecutionError(e.to_string()))?;

    let mut stdout_reader = child.stdout.take()
        .ok_or_else(|| PantherError::ExecutionError("stdout pipe unavailable".into()))?;
    let mut stderr_reader = child.stderr.take()
        .ok_or_else(|| PantherError::ExecutionError("stderr pipe unavailable".into()))?;

    let stdout_task = tokio::spawn(async move {
        let mut buf = String::new();
        let _ = stdout_reader.read_to_string(&mut buf).await;
        buf
    });
    let stderr_task = tokio::spawn(async move {
        let mut buf = String::new();
        let _ = stderr_reader.read_to_string(&mut buf).await;
        buf
    });

    match timeout(Duration::from_secs(TIMEOUT_SECS), child.wait()).await {
        Err(_) => {
            let _ = child.kill().await;
            stdout_task.abort();
            stderr_task.abort();
            Err(PantherError::ExecutionError("Execution timed out after 30 seconds".into()))
        }
        Ok(Err(e)) => Err(PantherError::ExecutionError(e.to_string())),
        Ok(Ok(status)) => {
            let stdout = stdout_task.await.unwrap_or_default();
            let stderr = stderr_task.await.unwrap_or_default();
            let mut combined = stdout;
            if !stderr.trim().is_empty() {
                combined.push_str(&stderr);
            }
            if !status.success() {
                let code = status.code().unwrap_or(-1);
                combined.push_str(&format!("\n[exit code: {}]", code));
            }
            Ok(combined)
        }
    }
}

fn unique_name(ext: &str) -> String {
    format!("panther_{}.{}", Uuid::new_v4().to_string().replace('-', ""), ext)
}

pub async fn run_powershell(code: &str, input: &str) -> PantherResult<String> {
    run_powershell_with_env(code, input, &[]).await
}

pub async fn run_powershell_with_env(code: &str, input: &str, env_vars: &[(&str, &str)]) -> PantherResult<String> {
    let script_path = std::env::temp_dir().join(unique_name("ps1"));
    tokio::fs::write(&script_path, code).await?;
    eprintln!("[panther:sandbox] powershell script: {}", script_path.display());

    let mut cmd = Command::new("powershell");
    cmd.args(["-ExecutionPolicy", "Bypass", "-NonInteractive", "-File", script_path.to_str().ok_or_else(|| PantherError::ExecutionError("script path is not valid UTF-8".into()))?]);
    let result = run_subprocess_path(&mut cmd, input, env_vars).await;
    eprintln!("[panther:sandbox] powershell output: {:?}", result);

    let _ = tokio::fs::remove_file(&script_path).await;
    result
}

pub async fn run_python(code: &str, input: &str) -> PantherResult<String> {
    run_python_with_env(code, input, &[]).await
}

pub async fn run_python_with_env(code: &str, input: &str, env_vars: &[(&str, &str)]) -> PantherResult<String> {
    let script_path = std::env::temp_dir().join(unique_name("py"));
    tokio::fs::write(&script_path, code).await?;
    eprintln!("[panther:sandbox] python script: {}", script_path.display());

    let mut cmd = Command::new("python");
    cmd.arg(script_path.to_str().ok_or_else(|| PantherError::ExecutionError("script path is not valid UTF-8".into()))?);
    let result = run_subprocess_path(&mut cmd, input, env_vars).await;
    eprintln!("[panther:sandbox] python output: {:?}", result);

    let _ = tokio::fs::remove_file(&script_path).await;
    result
}

pub async fn run_node(code: &str, input: &str) -> PantherResult<String> {
    run_node_with_env(code, input, &[]).await
}

pub async fn run_node_with_env(code: &str, input: &str, env_vars: &[(&str, &str)]) -> PantherResult<String> {
    let script_path = std::env::temp_dir().join(unique_name("js"));
    tokio::fs::write(&script_path, code).await?;
    eprintln!("[panther:sandbox] node script: {}", script_path.display());

    let mut cmd = Command::new("node");
    cmd.arg(script_path.to_str().ok_or_else(|| PantherError::ExecutionError("script path is not valid UTF-8".into()))?);
    let result = run_subprocess_path(&mut cmd, input, env_vars).await;
    eprintln!("[panther:sandbox] node output: {:?}", result);

    let _ = tokio::fs::remove_file(&script_path).await;
    result
}

pub async fn run_bash(code: &str, input: &str) -> PantherResult<String> {
    run_bash_with_env(code, input, &[]).await
}

pub async fn run_bash_with_env(code: &str, input: &str, env_vars: &[(&str, &str)]) -> PantherResult<String> {
    let script_path = std::env::temp_dir().join(unique_name("sh"));
    tokio::fs::write(&script_path, code).await?;

    let mut cmd = Command::new("bash");
    cmd.arg(script_path.to_str().ok_or_else(|| PantherError::ExecutionError("script path is not valid UTF-8".into()))?);
    let result = run_subprocess_path(&mut cmd, input, env_vars).await;

    let _ = tokio::fs::remove_file(&script_path).await;
    result
}

pub async fn run_rust(code: &str, input: &str) -> PantherResult<String> {
    run_rust_with_env(code, input, &[]).await
}

pub async fn run_rust_with_env(code: &str, input: &str, env_vars: &[(&str, &str)]) -> PantherResult<String> {
    let src_path = std::env::temp_dir().join(unique_name("rs"));
    let bin_path = src_path.with_extension("bin");

    tokio::fs::write(&src_path, code).await?;

    let compile = Command::new("rustc")
        .arg(&src_path)
        .arg("-o")
        .arg(&bin_path)
        .output()
        .await
        .map_err(|e| PantherError::ExecutionError(e.to_string()))?;

    let _ = tokio::fs::remove_file(&src_path).await;

    if !compile.status.success() {
        let err = String::from_utf8_lossy(&compile.stderr).to_string();
        return Err(PantherError::ExecutionError(err));
    }

    let mut run_cmd = Command::new(&bin_path);
    let result = run_subprocess_path(&mut run_cmd, input, env_vars).await;
    let _ = tokio::fs::remove_file(&bin_path).await;
    result
}
