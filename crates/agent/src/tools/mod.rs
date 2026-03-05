pub mod exec;
pub mod filesystem;
pub mod web;
pub mod message;
pub mod cron;
pub mod spawn;
pub mod skill;
pub mod mcp;
pub mod registry;
pub mod capture;
pub mod send_file;
pub mod clipboard;
pub mod system_info;

use std::pin::Pin;
use std::future::Future;
use serde_json::Value;

pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> Value;
    fn execute<'a>(&'a self, args: Value) -> Pin<Box<dyn Future<Output = String> + Send + 'a>>;
}
