use std::path::Path;

use shared::errors::{PantherError, PantherResult};
use teloxide::prelude::*;

pub async fn download_file(
    bot: &Bot,
    token: &str,
    file_id: &str,
    dest_path: &Path,
) -> PantherResult<()> {
    let file = bot
        .get_file(file_id)
        .await
        .map_err(|e| PantherError::TelegramError(e.to_string()))?;

    let url = format!("https://api.telegram.org/file/bot{}/{}", token, file.path);

    if let Some(parent) = dest_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let bytes = reqwest::get(&url)
        .await
        .map_err(|e| PantherError::TelegramError(e.to_string()))?
        .bytes()
        .await
        .map_err(|e| PantherError::TelegramError(e.to_string()))?;

    tokio::fs::write(dest_path, &bytes).await?;

    Ok(())
}
