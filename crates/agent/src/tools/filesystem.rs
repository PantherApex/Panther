use std::pin::Pin;
use std::future::Future;
use std::path::PathBuf;
use serde_json::json;

use super::Tool;

pub struct ReadFileTool;

impl Tool for ReadFileTool {
    fn name(&self) -> &str { "read_file" }
    fn description(&self) -> &str {
        "Read the contents of a file. Provide the full path."
    }
    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Absolute or home-relative path to the file" }
            },
            "required": ["path"]
        })
    }
    fn execute<'a>(&'a self, args: serde_json::Value) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
        Box::pin(async move {
            let path = match get_path(&args) {
                Ok(p) => p,
                Err(e) => return e,
            };
            match tokio::fs::read_to_string(&path).await {
                Ok(content) => content,
                Err(e) => format!("Error reading {}: {}", path.display(), e),
            }
        })
    }
}

pub struct WriteFileTool;

impl Tool for WriteFileTool {
    fn name(&self) -> &str { "write_file" }
    fn description(&self) -> &str {
        "Write content to a file, creating it and any parent directories if needed. Overwrites existing content."
    }
    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to the file to write" },
                "content": { "type": "string", "description": "Content to write" }
            },
            "required": ["path", "content"]
        })
    }
    fn execute<'a>(&'a self, args: serde_json::Value) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
        Box::pin(async move {
            let path = match get_path(&args) {
                Ok(p) => p,
                Err(e) => return e,
            };
            let content = match args.get("content").and_then(|v| v.as_str()) {
                Some(c) => c.to_string(),
                None => return "Error: missing 'content' argument".to_string(),
            };
            if let Some(parent) = path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            match tokio::fs::write(&path, &content).await {
                Ok(_) => format!("Written {} bytes to {}", content.len(), path.display()),
                Err(e) => format!("Error writing {}: {}", path.display(), e),
            }
        })
    }
}

pub struct EditFileTool;

impl Tool for EditFileTool {
    fn name(&self) -> &str { "edit_file" }
    fn description(&self) -> &str {
        "Edit a file by replacing a specific string with another. The old_str must match exactly and appear exactly once."
    }
    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to the file to edit" },
                "old_str": { "type": "string", "description": "The exact string to find and replace" },
                "new_str": { "type": "string", "description": "The replacement string" }
            },
            "required": ["path", "old_str", "new_str"]
        })
    }
    fn execute<'a>(&'a self, args: serde_json::Value) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
        Box::pin(async move {
            let path = match get_path(&args) {
                Ok(p) => p,
                Err(e) => return e,
            };
            let old_str = match args.get("old_str").and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None => return "Error: missing 'old_str'".to_string(),
            };
            let new_str = match args.get("new_str").and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None => return "Error: missing 'new_str'".to_string(),
            };
            let content = match tokio::fs::read_to_string(&path).await {
                Ok(c) => c,
                Err(e) => return format!("Error reading {}: {}", path.display(), e),
            };
            let count = content.matches(&old_str as &str).count();
            if count == 0 {
                return format!("Error: old_str not found in {}", path.display());
            }
            if count > 1 {
                return format!("Error: old_str appears {} times in {}. Must appear exactly once.", count, path.display());
            }
            let new_content = content.replacen(&old_str as &str, &new_str, 1);
            match tokio::fs::write(&path, &new_content).await {
                Ok(_) => format!("Successfully edited {}", path.display()),
                Err(e) => format!("Error writing {}: {}", path.display(), e),
            }
        })
    }
}

pub struct ListDirTool;

impl Tool for ListDirTool {
    fn name(&self) -> &str { "list_dir" }
    fn description(&self) -> &str {
        "List files and directories at a given path."
    }
    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to list" }
            },
            "required": ["path"]
        })
    }
    fn execute<'a>(&'a self, args: serde_json::Value) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
        Box::pin(async move {
            let path = match get_path(&args) {
                Ok(p) => p,
                Err(e) => return e,
            };
            let mut entries = match tokio::fs::read_dir(&path).await {
                Ok(e) => e,
                Err(e) => return format!("Error listing {}: {}", path.display(), e),
            };
            let mut lines = vec![format!("Contents of {}:", path.display())];
            let mut count = 0;
            while let Ok(Some(entry)) = entries.next_entry().await {
                let name = entry.file_name().to_string_lossy().to_string();
                let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
                if is_dir {
                    lines.push(format!("  {}/", name));
                } else {
                    lines.push(format!("  {}", name));
                }
                count += 1;
            }
            if count == 0 {
                lines.push("  (empty)".to_string());
            }
            lines.join("\n")
        })
    }
}

fn get_path(args: &serde_json::Value) -> Result<PathBuf, String> {
    let raw = args.get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Error: missing 'path' argument".to_string())?;

    let normalized = raw.replace('\\', "/");

    if normalized.starts_with("~/") || normalized == "~" {
        if let Some(home) = dirs::home_dir() {
            let rest = normalized.trim_start_matches("~/").trim_start_matches('~');
            return Ok(if rest.is_empty() { home } else { home.join(rest) });
        }
    }

    Ok(PathBuf::from(raw))
}
