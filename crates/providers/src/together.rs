use std::time::Duration;
use async_trait::async_trait;
use shared::errors::{PantherError, PantherResult};
use shared::types::{LLMProvider, LLMRequest, LLMResponse};
use crate::interface::ProviderInterface;
use crate::oai_common::{OAIRequest, OAIResponse, build_oai_messages, build_oai_tools, parse_oai_response};

#[derive(Clone)]
pub struct TogetherAIProvider {
    api_key: String,
    client: reqwest::Client,
}

impl TogetherAIProvider {
    pub fn new(api_key: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("Failed to build reqwest client");
        Self { api_key, client }
    }
}

#[async_trait]
impl ProviderInterface for TogetherAIProvider {
    async fn chat(&self, request: LLMRequest) -> PantherResult<LLMResponse> {
        if self.api_key.is_empty() {
            return Err(PantherError::LLMError("Together AI API key not configured".into()));
        }

        let body = OAIRequest {
            model: &request.model,
            messages: build_oai_messages(&request),
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            tools: build_oai_tools(&request),
        };

        let resp = self
            .client
            .post("https://api.together.xyz/v1/chat/completions")
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| PantherError::LLMError(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(PantherError::LLMError(format!("Together AI returned {}: {}", status, body)));
        }

        let parsed: OAIResponse = resp
            .json()
            .await
            .map_err(|e| PantherError::LLMError(format!("Together AI parse error: {}", e)))?;

        parse_oai_response(parsed, LLMProvider::TogetherAI)
    }
}
