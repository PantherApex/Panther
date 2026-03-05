use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use shared::errors::{PantherError, PantherResult};

#[async_trait]
pub trait TranscriptionProvider: Send + Sync {
    async fn transcribe(&self, file_path: &Path) -> PantherResult<String>;
}

#[derive(Deserialize)]
struct GroqTranscriptionResponse {
    text: String,
}

#[derive(Clone)]
pub struct GroqTranscriptionProvider {
    api_key: String,
    model: String,
    client: reqwest::Client,
}

impl GroqTranscriptionProvider {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("Failed to build reqwest client for transcription");
        Self {
            api_key: api_key.into(),
            model: model.into(),
            client,
        }
    }
}

fn mime_for_path(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(|s| s.to_lowercase()).as_deref() {
        Some("ogg") => "audio/ogg",
        Some("mp3") => "audio/mpeg",
        Some("mp4") => "audio/mp4",
        Some("m4a") => "audio/mp4",
        Some("wav") => "audio/wav",
        Some("webm") => "audio/webm",
        Some("flac") => "audio/flac",
        Some("opus") => "audio/ogg",
        _ => "application/octet-stream",
    }
}

#[async_trait]
impl TranscriptionProvider for GroqTranscriptionProvider {
    async fn transcribe(&self, file_path: &Path) -> PantherResult<String> {
        let bytes = tokio::fs::read(file_path).await.map_err(|e| {
            PantherError::ChannelError(format!("Failed to read audio file for transcription: {}", e))
        })?;

        let filename = file_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "audio.ogg".to_string());

        let mime = mime_for_path(file_path);

        let file_part = reqwest::multipart::Part::bytes(bytes)
            .file_name(filename)
            .mime_str(mime)
            .map_err(|e| PantherError::ChannelError(format!("Invalid MIME type for transcription: {}", e)))?;

        let form = reqwest::multipart::Form::new()
            .part("file", file_part)
            .text("model", self.model.clone())
            .text("response_format", "json");

        let response = self
            .client
            .post("https://api.groq.com/openai/v1/audio/transcriptions")
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await
            .map_err(|e| PantherError::ChannelError(format!("Groq transcription request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(PantherError::ChannelError(format!(
                "Groq transcription API returned {}: {}",
                status, body
            )));
        }

        let parsed: GroqTranscriptionResponse = response
            .json()
            .await
            .map_err(|e| PantherError::ChannelError(format!("Failed to parse Groq transcription response: {}", e)))?;

        let trimmed = parsed.text.trim().to_string();
        if trimmed.is_empty() {
            return Err(PantherError::ChannelError(
                "Groq transcription returned empty text".to_string(),
            ));
        }

        Ok(trimmed)
    }
}
