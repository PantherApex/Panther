use std::path::PathBuf;
use std::pin::Pin;
use std::future::Future;
use std::sync::Arc;

use reqwest::Client;
use serde_json::json;
use shared::channel::{Channel, FileKind};
use shared::errors::{PantherError, PantherResult};

pub struct DiscordChannel {
    client: Arc<Client>,
    bot_token: String,
}

impl DiscordChannel {
    pub fn new(bot_token: String) -> Self {
        Self {
            client: Arc::new(Client::new()),
            bot_token,
        }
    }

    fn auth_header(&self) -> String {
        format!("Bot {}", self.bot_token)
    }
}

impl Channel for DiscordChannel {
    fn name(&self) -> &str {
        "discord"
    }

    fn send<'a>(
        &'a self,
        chat_id: &'a str,
        text: &'a str,
    ) -> Pin<Box<dyn Future<Output = PantherResult<()>> + Send + 'a>> {
        Box::pin(async move {
            const LIMIT: usize = 2000;
            let chars: Vec<char> = text.chars().collect();
            for chunk in chars.chunks(LIMIT) {
                let part: String = chunk.iter().collect();
                let url = format!("https://discord.com/api/v10/channels/{}/messages", chat_id);
                let body = json!({ "content": part });
                let resp = self.client
                    .post(&url)
                    .header("Authorization", self.auth_header())
                    .header("Content-Type", "application/json")
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| PantherError::ChannelError(format!("Discord send failed: {}", e)))?;

                if !resp.status().is_success() {
                    let status = resp.status();
                    let err_body: String = resp.text().await.unwrap_or_default();
                    return Err(PantherError::ChannelError(format!(
                        "Discord API error {}: {}", status, err_body
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
        _kind: FileKind,
    ) -> Pin<Box<dyn Future<Output = PantherResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let bytes = tokio::fs::read(path).await
                .map_err(|e| PantherError::ChannelError(format!("Failed to read file: {}", e)))?;

            let filename = path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "file".to_string());

            let part = reqwest::multipart::Part::bytes(bytes).file_name(filename);
            let form = reqwest::multipart::Form::new().part("files[0]", part);

            let url = format!("https://discord.com/api/v10/channels/{}/messages", chat_id);
            let resp = self.client
                .post(&url)
                .header("Authorization", self.auth_header())
                .multipart(form)
                .send()
                .await
                .map_err(|e| PantherError::ChannelError(format!("Discord file send failed: {}", e)))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let err_body: String = resp.text().await.unwrap_or_default();
                return Err(PantherError::ChannelError(format!(
                    "Discord API error sending file {}: {}", status, err_body
                )));
            }
            Ok(())
        })
    }
}
