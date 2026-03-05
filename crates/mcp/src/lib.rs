use std::process::Stdio;
use std::sync::Arc;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

use shared::errors::{PantherError, PantherResult};
use shared::types::ToolDefinition;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    #[allow(dead_code)]
    jsonrpc: String,
    #[allow(dead_code)]
    id: Option<Value>,
    result: Option<Value>,
    error: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct McpToolInfo {
    pub name: String,
    pub description: Option<String>,
    #[serde(rename = "inputSchema")]
    pub input_schema: Option<Value>,
}

pub struct McpClient {
    server_name: String,
    stdin: Arc<Mutex<ChildStdin>>,
    stdout: Arc<Mutex<BufReader<ChildStdout>>>,
    _child: Arc<Mutex<Child>>,
    id_counter: Arc<Mutex<u64>>,
}

impl McpClient {
    pub async fn spawn(server_name: String, command: &str, args: &[String]) -> PantherResult<Self> {
        #[cfg(windows)]
        let mut child = {
            let mut all_args = vec!["/C".to_string(), command.to_string()];
            all_args.extend_from_slice(args);
            Command::new("cmd")
                .args(&all_args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| PantherError::ConfigError(format!(
                    "Failed to spawn MCP server '{}' ({}): {}", server_name, command, e
                )))?
        };

        #[cfg(not(windows))]
        let mut child = Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| PantherError::ConfigError(format!(
                "Failed to spawn MCP server '{}' ({}): {}", server_name, command, e
            )))?;

        let stdin = child.stdin.take()
            .ok_or_else(|| PantherError::ConfigError(format!(
                "MCP server '{}' has no stdin", server_name
            )))?;
        let stdout = child.stdout.take()
            .ok_or_else(|| PantherError::ConfigError(format!(
                "MCP server '{}' has no stdout", server_name
            )))?;

        let client = Self {
            server_name: server_name.clone(),
            stdin: Arc::new(Mutex::new(stdin)),
            stdout: Arc::new(Mutex::new(BufReader::new(stdout))),
            _child: Arc::new(Mutex::new(child)),
            id_counter: Arc::new(Mutex::new(0)),
        };

        client.initialize().await.map_err(|e| PantherError::ConfigError(format!(
            "MCP server '{}' initialization failed: {}", server_name, e
        )))?;

        Ok(client)
    }

    async fn next_id(&self) -> u64 {
        let mut counter = self.id_counter.lock().await;
        *counter += 1;
        *counter
    }

    async fn send_request(&self, method: &str, params: Option<Value>) -> PantherResult<Value> {
        let id = self.next_id().await;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id,
            method: method.to_string(),
            params,
        };

        let mut line = serde_json::to_string(&req)
            .map_err(|e| PantherError::SerializationError(e))?;
        line.push('\n');

        {
            let mut stdin = self.stdin.lock().await;
            stdin.write_all(line.as_bytes()).await
                .map_err(|e| PantherError::IoError(e))?;
            stdin.flush().await
                .map_err(|e| PantherError::IoError(e))?;
        }

        let mut response_line = String::new();
        {
            let mut stdout = self.stdout.lock().await;
            loop {
                response_line.clear();
                let n = stdout.read_line(&mut response_line).await
                    .map_err(|e| PantherError::IoError(e))?;
                if n == 0 {
                    return Err(PantherError::ConfigError(format!(
                        "MCP server '{}' closed connection", self.server_name
                    )));
                }
                let trimmed = response_line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                break;
            }
        }

        let resp: JsonRpcResponse = serde_json::from_str(response_line.trim())
            .map_err(|e| PantherError::ConfigError(format!(
                "MCP server '{}' sent invalid JSON: {} — raw: {}", self.server_name, e, response_line.trim()
            )))?;

        if let Some(err) = resp.error {
            return Err(PantherError::ConfigError(format!(
                "MCP server '{}' returned error: {}", self.server_name, err
            )));
        }

        resp.result.ok_or_else(|| PantherError::ConfigError(format!(
            "MCP server '{}' returned no result", self.server_name
        )))
    }

    async fn initialize(&self) -> PantherResult<()> {
        let params = json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "clientInfo": {
                "name": "panther",
                "version": "0.1.0"
            }
        });
        self.send_request("initialize", Some(params)).await?;
        self.send_notification("notifications/initialized").await?;
        Ok(())
    }

    async fn send_notification(&self, method: &str) -> PantherResult<()> {
        let notif = json!({
            "jsonrpc": "2.0",
            "method": method
        });
        let mut line = serde_json::to_string(&notif)
            .map_err(|e| PantherError::SerializationError(e))?;
        line.push('\n');
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(line.as_bytes()).await
            .map_err(|e| PantherError::IoError(e))?;
        stdin.flush().await
            .map_err(|e| PantherError::IoError(e))?;
        Ok(())
    }

    pub async fn list_tools(&self) -> PantherResult<Vec<McpToolInfo>> {
        let result = self.send_request("tools/list", None).await?;
        let tools = result["tools"].as_array()
            .ok_or_else(|| PantherError::ConfigError(format!(
                "MCP server '{}' tools/list returned no 'tools' array", self.server_name
            )))?;

        let mut out = Vec::new();
        for t in tools {
            if let Ok(info) = serde_json::from_value::<McpToolInfo>(t.clone()) {
                out.push(info);
            }
        }
        Ok(out)
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> PantherResult<String> {
        let params = json!({
            "name": name,
            "arguments": arguments
        });
        let result = self.send_request("tools/call", Some(params)).await?;

        if let Some(content) = result["content"].as_array() {
            let parts: Vec<String> = content.iter().filter_map(|item| {
                if item["type"].as_str() == Some("text") {
                    item["text"].as_str().map(|s| s.to_string())
                } else {
                    None
                }
            }).collect();
            if !parts.is_empty() {
                return Ok(parts.join("\n"));
            }
        }

        if let Some(text) = result.as_str() {
            return Ok(text.to_string());
        }

        Ok(serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string()))
    }

    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    pub fn to_tool_definitions(&self, tools: &[McpToolInfo]) -> Vec<ToolDefinition> {
        tools.iter().map(|t| {
            let params = t.input_schema.clone().unwrap_or_else(|| json!({
                "type": "object",
                "properties": {}
            }));
            let raw_name = format!("mcp_{}_{}", self.server_name, t.name);
            let safe_name: String = raw_name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
            ToolDefinition {
                name: safe_name,
                description: t.description.clone().unwrap_or_else(|| t.name.clone()),
                parameters: params,
            }
        }).collect()
    }
}

pub struct McpConnections {
    pub clients: HashMap<String, Arc<McpClient>>,
}

impl McpConnections {
    pub fn new() -> Self {
        Self { clients: HashMap::new() }
    }

    pub fn insert(&mut self, name: String, client: Arc<McpClient>) {
        self.clients.insert(name, client);
    }
}
