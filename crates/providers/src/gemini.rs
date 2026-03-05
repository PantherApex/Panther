use std::time::Duration;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use shared::errors::{PantherError, PantherResult};
use shared::types::{LLMProvider, LLMRequest, LLMResponse};
use crate::interface::ProviderInterface;

#[derive(Clone)]
pub struct GeminiProvider {
    api_key: String,
    client: reqwest::Client,
}

impl GeminiProvider {
    pub fn new(api_key: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("Failed to build reqwest client");
        Self { api_key, client }
    }
}

#[derive(Serialize)]
struct GeminiPart {
    text: String,
}

#[derive(Serialize)]
struct GeminiContent {
    parts: Vec<GeminiPart>,
}

#[derive(Serialize)]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
}

#[derive(Deserialize)]
struct GeminiRespPart {
    text: String,
}

#[derive(Deserialize)]
struct GeminiRespContent {
    parts: Vec<GeminiRespPart>,
}

#[derive(Deserialize)]
struct GeminiCandidate {
    content: GeminiRespContent,
}

#[derive(Deserialize)]
struct GeminiResponse {
    candidates: Vec<GeminiCandidate>,
}

#[async_trait]
impl ProviderInterface for GeminiProvider {
    async fn chat(&self, request: LLMRequest) -> PantherResult<LLMResponse> {
        if self.api_key.is_empty() {
            return Err(PantherError::LLMError("Gemini API key not configured".into()));
        }

        let prompt = request
            .messages
            .iter()
            .map(|m| {
                let text = m.content.as_deref().unwrap_or("");
                format!("{}: {}", m.role, text)
            })
            .collect::<Vec<_>>()
            .join("\n");

        let body = GeminiRequest {
            contents: vec![GeminiContent {
                parts: vec![GeminiPart { text: prompt }],
            }],
        };

        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            request.model, self.api_key
        );

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| PantherError::LLMError(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(PantherError::LLMError(format!(
                "Gemini returned {}: {}",
                status, body
            )));
        }

        let parsed: GeminiResponse = resp
            .json()
            .await
            .map_err(|e| PantherError::LLMError(format!("Gemini parse error: {}", e)))?;

        let content = parsed
            .candidates
            .into_iter()
            .next()
            .and_then(|c| c.content.parts.into_iter().next())
            .map(|p| p.text)
            .ok_or_else(|| PantherError::LLMError("Gemini returned empty candidates".into()))?;

        Ok(LLMResponse {
            content: Some(content),
            model: request.model,
            provider: LLMProvider::Gemini,
            tool_calls: None,
            finish_reason: None,
        })
    }
}
