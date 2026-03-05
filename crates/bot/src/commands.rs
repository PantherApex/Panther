use shared::types::LLMProvider;
use teloxide::prelude::*;
use teloxide::types::Message;

use crate::bot::{PantherBot, TELEGRAM_CHANNEL};

pub async fn handle_command(bot: Bot, msg: Message, panther: PantherBot) -> ResponseResult<()> {
    let text = match msg.text() {
        Some(t) => t,
        None => return Ok(()),
    };

    let mut parts = text.splitn(2, ' ');
    let command = parts.next().unwrap_or("");
    let _arg = parts.next().unwrap_or("").trim();

    let chat_id_str = msg.chat.id.0.to_string();

    let reply = match command {
        "/start" => {
            "Panther is online. I'm your personal AI agent. Just talk to me.".to_string()
        }

        "/status" => {
            format!("Active provider: {:?}\nPanther is running.", panther.agent.providers.active)
        }

        "/stop" => {
            let session_key = format!("{}:{}", TELEGRAM_CHANNEL, chat_id_str);
            let count = panther.agent.stop(&session_key).await;
            if count > 0 {
                format!("⏹ Stopped {} task(s).", count)
            } else {
                "No active task to stop.".to_string()
            }
        }

        "/new" => {
            match panther.agent.new_session(TELEGRAM_CHANNEL, &chat_id_str).await {
                Ok(_) => "🐆 New session started. Previous conversation archived to memory.".to_string(),
                Err(e) => format!("Failed to start new session: {}", e),
            }
        }

        "/clear" => {
            let session_key = format!("{}:{}", TELEGRAM_CHANNEL, chat_id_str);
            panther.agent.stop(&session_key).await;
            match panther.agent.session_store.clear(&session_key).await {
                Ok(_) => "Session cleared.".to_string(),
                Err(e) => format!("Failed to clear session: {}", e),
            }
        }

        "/help" => "\
/start — confirm Panther is online
/status — show active LLM provider
/new — archive conversation to memory and start fresh
/stop — stop the current running task
/clear — clear session without archiving
/help — show this message"
            .to_string(),

        _ => "Unknown command. Type /help to see available commands.".to_string(),
    };

    bot.send_message(msg.chat.id, reply).await?;
    Ok(())
}

fn _parse_provider(name: &str) -> Option<LLMProvider> {
    match name.to_lowercase().as_str() {
        "ollama" => Some(LLMProvider::Ollama),
        "openai" => Some(LLMProvider::OpenAI),
        "anthropic" => Some(LLMProvider::Anthropic),
        "openrouter" => Some(LLMProvider::OpenRouter),
        "gemini" => Some(LLMProvider::Gemini),
        "groq" => Some(LLMProvider::Groq),
        "mistral" => Some(LLMProvider::Mistral),
        "deepseek" => Some(LLMProvider::DeepSeek),
        "xai" => Some(LLMProvider::XAI),
        "together" => Some(LLMProvider::TogetherAI),
        "perplexity" => Some(LLMProvider::Perplexity),
        "cohere" => Some(LLMProvider::Cohere),
        _ => None,
    }
}
