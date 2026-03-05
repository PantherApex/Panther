use std::time::Duration;
use async_trait::async_trait;
use shared::errors::{PantherError, PantherResult};
use shared::types::{LLMProvider, LLMRequest, LLMResponse};
use crate::interface::ProviderInterface;
use crate::oai_common::{OAIFunction, OAIRequest, OAIResponse, OAITool, build_oai_messages, parse_oai_response};

#[derive(Clone)]
pub struct OpenAIProvider {
    api_key: String,
    client: reqwest::Client,
}

impl OpenAIProvider {
    pub fn new(api_key: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("Failed to build reqwest client");
        Self { api_key, client }
    }
}

#[async_trait]
impl ProviderInterface for OpenAIProvider {
    async fn chat(&self, request: LLMRequest) -> PantherResult<LLMResponse> {
        if self.api_key.is_empty() {
            return Err(PantherError::LLMError("OpenAI API key not configured".into()));
        }

        let tools: Option<Vec<OAITool>> = request.tools.as_ref().map(|defs| {
            defs.iter().map(|d| OAITool {
                kind: "function",
                function: OAIFunction {
                    name: &d.name,
                    description: &d.description,
                    parameters: &d.parameters,
                },
            }).collect()
        });

        let body = OAIRequest {
            model: &request.model,
            messages: build_oai_messages(&request),
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            tools,
        };

        let resp = self
            .client
            .post("https://api.openai.com/v1/chat/completions")
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| PantherError::LLMError(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(PantherError::LLMError(format!("OpenAI returned {}: {}", status, body)));
        }

        let parsed: OAIResponse = resp
            .json()
            .await
            .map_err(|e| PantherError::LLMError(format!("OpenAI parse error: {}", e)))?;

        parse_oai_response(parsed, LLMProvider::OpenAI)
    }
}
