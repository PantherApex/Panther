use std::collections::HashSet;
use std::sync::Arc;
use serenity::async_trait;
use serenity::model::channel::Message;
use serenity::model::gateway::Ready;
use serenity::prelude::*;

use shared::bus::{InboundMessage, MessageBus};
use shared::errors::PantherResult;

pub const DISCORD_CHANNEL: &str = "discord";

struct DiscordHandler {
    bus: MessageBus,
    bot_user_id: Arc<tokio::sync::Mutex<Option<u64>>>,
    allow_from: Arc<HashSet<String>>,
}

impl DiscordHandler {
    fn is_allowed(&self, user_id: u64, username: &str) -> bool {
        if self.allow_from.is_empty() {
            return true;
        }
        self.allow_from.contains(&user_id.to_string())
            || self.allow_from.contains(username)
    }
}

#[async_trait]
impl EventHandler for DiscordHandler {
    async fn ready(&self, _ctx: Context, ready: Ready) {
        *self.bot_user_id.lock().await = Some(ready.user.id.get());
        eprintln!("[panther:discord] connected as {}", ready.user.name);
    }

    async fn message(&self, ctx: Context, msg: Message) {
        let bot_id = *self.bot_user_id.lock().await;
        if Some(msg.author.id.get()) == bot_id || msg.author.bot {
            return;
        }

        if !self.is_allowed(msg.author.id.get(), &msg.author.name) {
            return;
        }

        let channel_id = msg.channel_id.get().to_string();

        let _ = msg.channel_id.broadcast_typing(&ctx.http).await;

        let inbound = match build_discord_inbound(&msg, channel_id.clone()).await {
            Some(m) => m,
            None => return,
        };

        self.bus.publish_inbound(inbound).await;
    }
}

async fn build_discord_inbound(msg: &Message, channel_id: String) -> Option<InboundMessage> {
    if let Some(attachment) = msg.attachments.first() {
        let is_image = attachment.content_type.as_deref()
            .map(|ct| ct.starts_with("image/"))
            .unwrap_or(false);

        if is_image {
            if let Ok(resp) = reqwest::get(&attachment.url).await {
                if resp.status().is_success() {
                    if let Ok(bytes) = resp.bytes().await {
                        use base64::Engine as _;
                        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                        let mime = attachment.content_type.clone()
                            .unwrap_or_else(|| "image/jpeg".to_string());
                        let content = if msg.content.is_empty() {
                            "[image attached]".to_string()
                        } else {
                            msg.content.clone()
                        };
                        return Some(InboundMessage {
                            channel: DISCORD_CHANNEL.to_string(),
                            sender_id: msg.author.id.get().to_string(),
                            chat_id: channel_id,
                            content,
                            media_path: None,
                            image_b64: Some((b64, mime)),
                            session_key_override: None,
                        });
                    }
                }
            }
        } else {
            let content = if msg.content.is_empty() {
                format!("[attachment: {}]", attachment.filename)
            } else {
                format!("{} [attachment: {}]", msg.content, attachment.filename)
            };
            return Some(InboundMessage {
                channel: DISCORD_CHANNEL.to_string(),
                sender_id: msg.author.id.get().to_string(),
                chat_id: channel_id,
                content,
                media_path: None,
                image_b64: None,
                session_key_override: None,
            });
        }
    }

    if msg.content.is_empty() {
        return None;
    }

    Some(InboundMessage {
        channel: DISCORD_CHANNEL.to_string(),
        sender_id: msg.author.id.get().to_string(),
        chat_id: channel_id,
        content: msg.content.clone(),
        media_path: None,
        image_b64: None,
        session_key_override: None,
    })
}

pub struct DiscordBot {
    token: String,
    bus: MessageBus,
    allow_from: Vec<String>,
}

impl DiscordBot {
    pub fn new(token: String, bus: MessageBus, allow_from: Vec<String>) -> Self {
        Self { token, bus, allow_from }
    }

    pub async fn run(self) -> PantherResult<()> {
        let intents = GatewayIntents::GUILD_MESSAGES
            | GatewayIntents::DIRECT_MESSAGES
            | GatewayIntents::MESSAGE_CONTENT;

        let handler = DiscordHandler {
            bus: self.bus,
            bot_user_id: Arc::new(tokio::sync::Mutex::new(None)),
            allow_from: Arc::new(self.allow_from.into_iter().collect()),
        };

        let mut client = Client::builder(&self.token, intents)
            .event_handler(handler)
            .await
            .map_err(|e| shared::errors::PantherError::ConfigError(
                format!("Discord client creation failed: {}", e)
            ))?;

        client.start().await.map_err(|e| shared::errors::PantherError::ConfigError(
            format!("Discord client error: {}", e)
        ))?;

        Ok(())
    }
}
