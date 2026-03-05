use std::path::PathBuf;
use std::pin::Pin;
use std::future::Future;
use std::sync::Arc;

use reqwest::Client;
use serde_json::json;
use shared::channel::{Channel, FileKind};
use shared::errors::{PantherError, PantherResult};

pub struct TelegramChannel {
    client: Arc<Client>,
    bot_token: String,
}

impl TelegramChannel {
    pub fn new(bot_token: String) -> Self {
        Self {
            client: Arc::new(Client::new()),
            bot_token,
        }
    }
}

impl Channel for TelegramChannel {
    fn name(&self) -> &str {
        "telegram"
    }

    fn send<'a>(
        &'a self,
        chat_id: &'a str,
        text: &'a str,
    ) -> Pin<Box<dyn Future<Output = PantherResult<()>> + Send + 'a>> {
        Box::pin(async move {
            const LIMIT: usize = 4096;
            let chars: Vec<char> = text.chars().collect();
            for chunk in chars.chunks(LIMIT) {
                let part: String = chunk.iter().collect();
                let url = format!("https://api.telegram.org/bot{}/sendMessage", self.bot_token);
                let body = json!({ "chat_id": chat_id, "text": part });
                let resp = self.client.post(&url).json(&body).send().await
                    .map_err(|e| PantherError::ChannelError(format!("Telegram send failed: {}", e)))?;
                if !resp.status().is_success() {
                    return Err(PantherError::ChannelError(format!(
                        "Telegram API error: {}", resp.status()
                    )));
                }
            }
            Ok(())
        })
    }

    fn send_file<'a>(
        &'a self,
        chat_id: &'a str,
        path: &'a PathBuf,
        kind: FileKind,
    ) -> Pin<Box<dyn Future<Output = PantherResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let bytes = tokio::fs::read(path).await
                .map_err(|e| PantherError::ChannelError(format!("Failed to read file: {}", e)))?;

            let filename = path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "file".to_string());

            let part = reqwest::multipart::Part::bytes(bytes).file_name(filename);

            let (endpoint, field) = match kind {
                FileKind::Photo => ("sendPhoto", "photo"),
                FileKind::Video => ("sendVideo", "video"),
                FileKind::Audio => ("sendAudio", "audio"),
                FileKind::Document => ("sendDocument", "document"),
            };

            let form = reqwest::multipart::Form::new()
                .text("chat_id", chat_id.to_string())
                .part(field, part);

            let url = format!("https://api.telegram.org/bot{}/{}", self.bot_token, endpoint);
            let resp = self.client.post(&url).multipart(form).send().await
                .map_err(|e| PantherError::ChannelError(format!("Telegram file send failed: {}", e)))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let err_body: String = resp.text().await.unwrap_or_default();
                return Err(PantherError::ChannelError(format!(
                    "Telegram API error sending file {}: {}", status, err_body
                )));
            }
            Ok(())
        })
    }

    fn supports_live_status(&self) -> bool {
        true
    }

    fn send_status<'a>(
        &'a self,
        chat_id: &'a str,
        text: &'a str,
    ) -> Pin<Box<dyn Future<Output = PantherResult<Option<u64>>> + Send + 'a>> {
        Box::pin(async move {
            let url = format!("https://api.telegram.org/bot{}/sendMessage", self.bot_token);
            let body = json!({ "chat_id": chat_id, "text": text });
            let resp = self.client.post(&url).json(&body).send().await
                .map_err(|e| PantherError::ChannelError(format!("Telegram status send failed: {}", e)))?;
            if !resp.status().is_success() {
                return Ok(None);
            }
            let payload: serde_json::Value = resp.json().await
                .map_err(|e| PantherError::ChannelError(format!("Telegram response parse failed: {}", e)))?;
            Ok(payload["result"]["message_id"].as_u64())
        })
    }

    fn edit_status<'a>(
        &'a self,
        chat_id: &'a str,
        message_id: u64,
        text: &'a str,
    ) -> Pin<Box<dyn Future<Output = PantherResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let url = format!("https://api.telegram.org/bot{}/editMessageText", self.bot_token);
            let body = json!({
                "chat_id": chat_id,
                "message_id": message_id,
                "text": text
            });
            let resp = self.client.post(&url).json(&body).send().await
                .map_err(|e| PantherError::ChannelError(format!("Telegram edit failed: {}", e)))?;
            if !resp.status().is_success() {
                let err_body = resp.text().await.unwrap_or_default();
                let is_benign = err_body.contains("message is not modified")
                    || err_body.contains("message to edit not found")
                    || err_body.contains("MESSAGE_ID_INVALID");
                if !is_benign {
                    return Err(PantherError::ChannelError(format!(
                        "Telegram editMessageText failed: {}", err_body
                    )));
                }
            }
            Ok(())
        })
    }

    fn delete_status<'a>(
        &'a self,
        chat_id: &'a str,
        message_id: u64,
    ) -> Pin<Box<dyn Future<Output = PantherResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let url = format!("https://api.telegram.org/bot{}/deleteMessage", self.bot_token);
            let body = json!({
                "chat_id": chat_id,
                "message_id": message_id
            });
            let resp = self.client.post(&url).json(&body).send().await
                .map_err(|e| PantherError::ChannelError(format!("Telegram delete failed: {}", e)))?;
            if !resp.status().is_success() {
                let err_body = resp.text().await.unwrap_or_default();
                let is_benign = err_body.contains("message to delete not found")
                    || err_body.contains("message can\'t be deleted")
                    || err_body.contains("MESSAGE_ID_INVALID");
                if !is_benign {
                    return Err(PantherError::ChannelError(format!(
                        "Telegram deleteMessage failed: {}", err_body
                    )));
                }
            }
            Ok(())
        })
    }
}
