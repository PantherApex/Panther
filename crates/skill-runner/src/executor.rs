use serde::{Deserialize, Serialize};
use shared::errors::PantherResult;

use crate::sandbox;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub output: String,
    pub success: bool,
    pub language: String,
    pub duration_ms: u64,
}

pub async fn execute(language: &str, code: &str, input: &str) -> PantherResult<ExecutionResult> {
    let start = std::time::Instant::now();
    let output = route(language, code, input).await?;
    let duration_ms = start.elapsed().as_millis() as u64;
    let success = !output.contains("[exit code:");
    Ok(ExecutionResult {
        output,
        success,
        language: language.to_string(),
        duration_ms,
    })
}

pub async fn execute_with_env(
    language: &str,
    code: &str,
    input: &str,
    env_vars: &[(&str, &str)],
) -> PantherResult<ExecutionResult> {
    let start = std::time::Instant::now();
    let output = route_with_env(language, code, input, env_vars).await?;
    let duration_ms = start.elapsed().as_millis() as u64;
    let success = !output.contains("[exit code:");
    Ok(ExecutionResult {
        output,
        success,
        language: language.to_string(),
        duration_ms,
    })
}

async fn route(language: &str, code: &str, input: &str) -> PantherResult<String> {
    match language.to_lowercase().as_str() {
        "python" => sandbox::run_python(code, input).await,
        "javascript" | "js" => sandbox::run_node(code, input).await,
        "bash" | "sh" => sandbox::run_bash(code, input).await,
        "powershell" | "ps1" => sandbox::run_powershell(code, input).await,
        "rust" => sandbox::run_rust(code, input).await,
        other => Err(shared::errors::PantherError::SkillError(format!("Unsupported language: {}", other))),
    }
}

async fn route_with_env(language: &str, code: &str, input: &str, env_vars: &[(&str, &str)]) -> PantherResult<String> {
    match language.to_lowercase().as_str() {
        "python" => sandbox::run_python_with_env(code, input, env_vars).await,
        "javascript" | "js" => sandbox::run_node_with_env(code, input, env_vars).await,
        "bash" | "sh" => sandbox::run_bash_with_env(code, input, env_vars).await,
        "powershell" | "ps1" => sandbox::run_powershell_with_env(code, input, env_vars).await,
        "rust" => sandbox::run_rust_with_env(code, input, env_vars).await,
        other => Err(shared::errors::PantherError::SkillError(format!("Unsupported language: {}", other))),
    }
}
