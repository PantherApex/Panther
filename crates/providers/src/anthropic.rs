use std::time::Duration;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use shared::errors::{PantherError, PantherResult};
use shared::types::{LLMProvider, LLMRequest, LLMResponse, ToolCall, ToolDefinition};
use crate::interface::ProviderInterface;

#[derive(Clone)]
pub struct AnthropicProvider {
    api_key: String,
    client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(api_key: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .expect("Failed to build reqwest client");
        Self { api_key, client }
    }
}

#[derive(Serialize)]
struct AnthropicTool<'a> {
    name: &'a str,
    description: &'a str,
    input_schema: &'a Value,
}

#[derive(Serialize)]
struct AnthropicTextBlock {
    #[serde(rename = "type")]
    kind: &'static str,
    text: String,
}

#[derive(Serialize)]
struct AnthropicToolResultBlock {
    #[serde(rename = "type")]
    kind: &'static str,
    tool_use_id: String,
    content: String,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicImageSource {
    Base64 {
        media_type: String,
        data: String,
    },
}

#[derive(Serialize)]
struct AnthropicImageBlock {
    #[serde(rename = "type")]
    kind: &'static str,
    source: AnthropicImageSource,
}

#[derive(Serialize)]
#[serde(untagged)]
#[allow(dead_code)]
enum AnthropicContentBlock {
    Text(AnthropicTextBlock),
    ToolResult(AnthropicToolResultBlock),
    Image(AnthropicImageBlock),
}

#[derive(Serialize)]
#[serde(untagged)]
enum AnthropicMessageContent {
    Text(String),
    Blocks(Vec<AnthropicContentBlock>),
}

#[derive(Serialize)]
struct AnthropicMessage {
    role: String,
    content: AnthropicMessageContent,
}

#[derive(Serialize)]
struct AnthropicRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicTool<'a>>>,
}

#[derive(Deserialize)]
struct AnthropicTextContent {
    text: String,
}

#[derive(Deserialize)]
struct AnthropicToolUseContent {
    id: String,
    name: String,
    input: Value,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum AnthropicResponseBlock {
    #[serde(rename = "text")]
    Text(AnthropicTextContent),
    #[serde(rename = "tool_use")]
    ToolUse(AnthropicToolUseContent),
    #[serde(rename = "thinking")]
    Thinking(()),
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicResponseBlock>,
    model: String,
    stop_reason: Option<String>,
}

#[async_trait]
impl ProviderInterface for AnthropicProvider {
    async fn chat(&self, request: LLMRequest) -> PantherResult<LLMResponse> {
        if self.api_key.is_empty() {
            return Err(PantherError::LLMError("Anthropic API key not configured".into()));
        }

        let mut system: Option<String> = None;
        let mut messages: Vec<AnthropicMessage> = Vec::new();

        for msg in &request.messages {
            match msg.role.as_str() {
                "system" => {
                    system = msg.content.clone();
                }
                "tool" => {
                    let tool_use_id = msg.tool_call_id.clone().unwrap_or_default();
                    let content_text = msg.content.clone().unwrap_or_default();
                    messages.push(AnthropicMessage {
                        role: "user".to_string(),
                        content: AnthropicMessageContent::Blocks(vec![
                            AnthropicContentBlock::ToolResult(AnthropicToolResultBlock {
                                kind: "tool_result",
                                tool_use_id,
                                content: content_text,
                            }),
                        ]),
                    });
                }
                role => {
                    let content = match (&msg.content, &msg.image_data) {
                        (Some(text), Some((b64, mime))) => {
                            AnthropicMessageContent::Blocks(vec![
                                AnthropicContentBlock::Image(AnthropicImageBlock {
                                    kind: "image",
                                    source: AnthropicImageSource::Base64 {
                                        media_type: mime.clone(),
                                        data: b64.clone(),
                                    },
                                }),
                                AnthropicContentBlock::Text(AnthropicTextBlock {
                                    kind: "text",
                                    text: text.clone(),
                                }),
                            ])
                        }
                        (Some(text), None) => AnthropicMessageContent::Text(text.clone()),
                        (None, _) => AnthropicMessageContent::Text(String::new()),
                    };
                    messages.push(AnthropicMessage {
                        role: role.to_string(),
                        content,
                    });
                }
            }
        }

        let tools: Option<Vec<AnthropicTool>> = request.tools.as_ref().map(|defs| {
            defs.iter()
                .map(|d: &ToolDefinition| AnthropicTool {
                    name: &d.name,
                    description: &d.description,
                    input_schema: &d.parameters,
                })
                .collect()
        });

        let body = AnthropicRequest {
            model: &request.model,
            max_tokens: request.max_tokens.unwrap_or(8096),
            messages,
            system,
            tools,
        };

        let resp = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| PantherError::LLMError(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(PantherError::LLMError(format!(
                "Anthropic returned {}: {}",
                status, body
            )));
        }

        let parsed: AnthropicResponse = resp
            .json()
            .await
            .map_err(|e| PantherError::LLMError(format!("Anthropic parse error: {}", e)))?;

        let mut text_parts: Vec<String> = Vec::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        for block in parsed.content {
            match block {
                AnthropicResponseBlock::Text(t) => {
                    if !t.text.is_empty() {
                        text_parts.push(t.text);
                    }
                }
                AnthropicResponseBlock::ToolUse(t) => {
                    tool_calls.push(ToolCall {
                        call_id: t.id,
                        name: t.name,
                        arguments: t.input,
                    });
                }
                AnthropicResponseBlock::Thinking(_) => {}
            }
        }

        let content = if text_parts.is_empty() {
            None
        } else {
            Some(text_parts.join("\n"))
        };

        Ok(LLMResponse {
            content,
            model: parsed.model,
            provider: LLMProvider::Anthropic,
            tool_calls: if tool_calls.is_empty() { None } else { Some(tool_calls) },
            finish_reason: parsed.stop_reason,
        })
    }
}
