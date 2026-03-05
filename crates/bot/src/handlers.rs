use std::path::PathBuf;

use shared::bus::InboundMessage;
use shared::errors::PantherError;
use teloxide::prelude::*;
use teloxide::types::Message;

use crate::bot::{PantherBot, TELEGRAM_CHANNEL};
use crate::{commands, downloader};

pub async fn handle_message(bot: Bot, msg: Message, panther: PantherBot) -> ResponseResult<()> {
    let sender_id = msg.from()
        .and_then(|u| u.username.as_deref())
        .map(|s| s.to_string())
        .or_else(|| msg.from().map(|u| u.id.to_string()))
        .unwrap_or_default();

    if !panther.is_allowed(&sender_id) {
        return Ok(());
    }

    if let Some(user) = msg.from() {
        if let Some(username) = &user.username {
            let chat_id = msg.chat.id.0;
            panther.known_chats.write().await.insert(username.to_lowercase(), chat_id);
            panther.memory.save_known_chats(&*panther.known_chats.read().await).await;
        }
    }

    if let Some(text) = msg.text() {
        if text.starts_with('/') {
            return commands::handle_command(bot, msg, panther).await;
        }
    }

    let chat_id_str = msg.chat.id.0.to_string();

    let _ = bot.send_chat_action(msg.chat.id, teloxide::types::ChatAction::Typing).await;

    let inbound = match build_inbound(&bot, &msg, &panther, chat_id_str.clone()).await {
        Ok(Some(m)) => m,
        Ok(None) => return Ok(()),
        Err(e) => {
            let safe = redact_token(&format!("Something went wrong: {}", e), &panther.token);
            bot.send_message(msg.chat.id, safe).await?;
            return Ok(());
        }
    };

    panther.bus.publish_inbound(inbound).await;

    Ok(())
}

pub(crate) async fn build_inbound(
    bot: &Bot,
    msg: &Message,
    panther: &PantherBot,
    chat_id_str: String,
) -> Result<Option<InboundMessage>, PantherError> {
    let temp_dir = dirs::home_dir()
        .ok_or_else(|| PantherError::ConfigError("Cannot determine home directory".into()))?
        .join(".panther")
        .join("temp");

    if let Some(text) = msg.text() {
        return Ok(Some(InboundMessage {
            channel: TELEGRAM_CHANNEL.to_string(),
            sender_id: msg.from().map(|u| u.id.to_string()).unwrap_or_default(),
            chat_id: chat_id_str,
            content: text.to_string(),
            media_path: None,
            image_b64: None,
            session_key_override: None,
        }));
    }

    if let Some(voice) = msg.voice() {
        let path = temp_dir.join(format!("{}.ogg", voice.file.id));
        downloader::download_file(bot, &panther.token, &voice.file.id, &path).await?;
        let content = transcribe_audio_file(&path, panther).await;
        return Ok(Some(InboundMessage {
            channel: TELEGRAM_CHANNEL.to_string(),
            sender_id: msg.from().map(|u| u.id.to_string()).unwrap_or_default(),
            chat_id: chat_id_str,
            content,
            media_path: Some(path.to_string_lossy().to_string()),
            image_b64: None,
            session_key_override: None,
        }));
    }

    if let Some(audio) = msg.audio() {
        let ext = audio.file_name.as_deref()
            .and_then(|n| std::path::Path::new(n).extension())
            .and_then(|e| e.to_str())
            .unwrap_or("mp3");
        let path = temp_dir.join(format!("{}.{}", audio.file.id, ext));
        downloader::download_file(bot, &panther.token, &audio.file.id, &path).await?;
        let content = transcribe_audio_file(&path, panther).await;
        return Ok(Some(InboundMessage {
            channel: TELEGRAM_CHANNEL.to_string(),
            sender_id: msg.from().map(|u| u.id.to_string()).unwrap_or_default(),
            chat_id: chat_id_str,
            content,
            media_path: Some(path.to_string_lossy().to_string()),
            image_b64: None,
            session_key_override: None,
        }));
    }

    if let Some(doc) = msg.document() {
        let filename = doc.file_name.as_deref().unwrap_or("file");
        let path = temp_dir.join(format!("{}_{}", doc.file.id, filename));
        downloader::download_file(bot, &panther.token, &doc.file.id, &path).await?;
        return Ok(Some(InboundMessage {
            channel: TELEGRAM_CHANNEL.to_string(),
            sender_id: msg.from().map(|u| u.id.to_string()).unwrap_or_default(),
            chat_id: chat_id_str,
            content: format!("[document: {}]", filename),
            media_path: Some(path.to_string_lossy().to_string()),
            image_b64: None,
            session_key_override: None,
        }));
    }

    if let Some(photos) = msg.photo() {
        if let Some(photo) = photos.iter().max_by_key(|p| p.width * p.height) {
            let path = temp_dir.join(format!("{}.jpg", photo.file.id));
            downloader::download_file(bot, &panther.token, &photo.file.id, &path).await?;
            let bytes = tokio::fs::read(&path).await
                .map_err(|e| PantherError::ConfigError(format!("Failed to read image: {}", e)))?;
            use base64::Engine as _;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            return Ok(Some(InboundMessage {
                channel: TELEGRAM_CHANNEL.to_string(),
                sender_id: msg.from().map(|u| u.id.to_string()).unwrap_or_default(),
                chat_id: chat_id_str,
                content: "[photo attached]".to_string(),
                media_path: None,
                image_b64: Some((b64, "image/jpeg".to_string())),
                session_key_override: None,
            }));
        }
    }

    Ok(None)
}

async fn transcribe_audio_file(path: &PathBuf, panther: &PantherBot) -> String {
    match &panther.transcription {
        None => "[voice message — voice transcription is not configured. Set groq_transcription_key or groq_key in your config to enable it]".to_string(),
        Some(provider) => match provider.transcribe(path).await {
            Ok(transcript) => format!("[Voice]: {}", transcript),
            Err(e) => {
                let summary = summarize_transcription_error(&e.to_string());
                format!("[voice message — transcription failed: {}]", summary)
            }
        },
    }
}

fn summarize_transcription_error(err: &str) -> String {
    if err.contains("401") || err.contains("Unauthorized") || err.contains("Invalid API key") {
        return "invalid API key".to_string();
    }
    if err.contains("429") || err.contains("rate limit") || err.contains("Rate limit") {
        return "rate limit exceeded, try again later".to_string();
    }
    if err.contains("413") || err.contains("too large") || err.contains("File too large") {
        return "audio file too large for transcription".to_string();
    }
    if err.contains("connection") || err.contains("connect") || err.contains("timeout") {
        return "network error".to_string();
    }
    if err.len() > 80 {
        format!("{}...", &err[..80])
    } else {
        err.to_string()
    }
}

fn redact_token(text: &str, token: &str) -> String {
    if token.is_empty() {
        return text.to_string();
    }
    text.replace(token, "[REDACTED]")
}

