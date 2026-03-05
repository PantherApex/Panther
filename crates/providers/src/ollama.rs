use std::time::Duration;
use async_trait::async_trait;
use serde::Serialize;
use shared::errors::{PantherError, PantherResult};
use shared::types::{LLMProvider, LLMRequest, LLMResponse};
use crate::interface::ProviderInterface;
use crate::oai_common::{OAIResponse, build_oai_messages, build_oai_tools, parse_oai_response};

#[derive(Clone)]
pub struct OllamaProvider {
    host: String,
    client: reqwest::Client,
}

impl OllamaProvider {
    pub fn new(host: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .expect("Failed to build reqwest client");
        Self { host, client }
    }
}

#[derive(Serialize)]
struct OllamaRequest<'a> {
    model: &'a str,
    messages: Vec<crate::oai_common::OAIMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<crate::oai_common::OAITool<'a>>>,
}

#[async_trait]
impl ProviderInterface for OllamaProvider {
    async fn chat(&self, request: LLMRequest) -> PantherResult<LLMResponse> {
        let body = OllamaRequest {
            model: &request.model,
            messages: build_oai_messages(&request),
            stream: false,
            tools: build_oai_tools(&request),
        };

        let resp = self
            .client
            .post(format!("{}/api/chat", self.host))
            .json(&body)
            .send()
            .await
            .map_err(|e| PantherError::LLMError(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(PantherError::LLMError(format!("Ollama returned {}: {}", status, body)));
        }

        let parsed: OAIResponse = resp
            .json()
            .await
            .map_err(|e| PantherError::LLMError(format!("Ollama parse error: {}", e)))?;

        parse_oai_response(parsed, LLMProvider::Ollama)
    }
}
