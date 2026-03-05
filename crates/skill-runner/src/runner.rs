use memory::MemoryStore;
use shared::errors::PantherResult;

use crate::executor::ExecutionResult;

#[derive(Clone)]
pub struct SkillRunner {
    _memory: MemoryStore,
}

impl SkillRunner {
    pub fn new(memory: MemoryStore) -> Self {
        Self { _memory: memory }
    }

    pub async fn run_raw(
        &self,
        language: &str,
        code: &str,
        input: &str,
    ) -> PantherResult<ExecutionResult> {
        let mut result = crate::executor::execute(language, code, input).await?;
        result.success = determine_success(&result.output, result.success);
        Ok(result)
    }

    pub async fn run_raw_with_env(
        &self,
        language: &str,
        code: &str,
        input: &str,
        env_vars: Vec<(String, String)>,
    ) -> PantherResult<ExecutionResult> {
        let borrowed: Vec<(&str, &str)> = env_vars.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let mut result = crate::executor::execute_with_env(language, code, input, &borrowed).await?;
        result.success = determine_success(&result.output, result.success);
        Ok(result)
    }
}

fn determine_success(output: &str, exit_success: bool) -> bool {
    if !exit_success {
        return false;
    }

    let last_lines: Vec<String> = output
        .lines()
        .rev()
        .filter(|l| !l.trim().is_empty())
        .take(5)
        .map(|l| l.to_lowercase())
        .collect();

    let terminal_failure_signals = [
        "exit code:",
        "exception",
        "error:",
        "fatal:",
        "traceback",
        "access denied",
        "is not recognized",
        "cannot be found",
        "terminatingerror",
    ];

    if last_lines.iter().any(|line| {
        terminal_failure_signals.iter().any(|sig| line.contains(sig))
    }) {
        return false;
    }

    true
}
