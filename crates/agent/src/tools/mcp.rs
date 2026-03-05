use std::pin::Pin;
use std::future::Future;
use std::sync::Arc;
use serde_json::Value;
use mcp::McpClient;
use super::Tool;

pub struct McpTool {
    client: Arc<McpClient>,
    tool_name: String,
    tool_description: String,
    tool_parameters: Value,
    full_name: String,
}

impl McpTool {
    pub fn new(
        client: Arc<McpClient>,
        tool_name: String,
        tool_description: String,
        tool_parameters: Value,
    ) -> Self {
        let raw = format!("mcp_{}_{}", client.server_name(), tool_name);
        let full_name = raw.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
        Self {
            client,
            tool_name,
            tool_description,
            tool_parameters,
            full_name,
        }
    }
}

impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.full_name
    }

    fn description(&self) -> &str {
        &self.tool_description
    }

    fn parameters(&self) -> Value {
        self.tool_parameters.clone()
    }

    fn execute<'a>(&'a self, args: Value) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
        Box::pin(async move {
            match self.client.call_tool(&self.tool_name, args).await {
                Ok(result) => result,
                Err(e) => format!("MCP tool '{}' error: {}", self.full_name, e),
            }
        })
    }
}
