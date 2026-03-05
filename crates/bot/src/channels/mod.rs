pub mod telegram;
pub mod discord;
pub mod slack;
pub mod email;
pub mod matrix;
pub mod cli;

pub use telegram::TelegramChannel;
pub use discord::DiscordChannel;
pub use slack::SlackChannel;
pub use email::{EmailChannel, EmailConfig};
pub use matrix::{MatrixChannel, MatrixConfig, MatrixGroupPolicy};
pub use cli::CliChannel;
