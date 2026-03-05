use std::path::PathBuf;
use std::pin::Pin;
use std::future::Future;
use crate::errors::PantherResult;

#[derive(Debug, Clone, PartialEq)]
pub enum FileKind {
    Photo,
    Video,
    Document,
    Audio,
}

pub trait Channel: Send + Sync {
    fn name(&self) -> &str;

    fn send<'a>(
        &'a self,
        chat_id: &'a str,
        text: &'a str,
    ) -> Pin<Box<dyn Future<Output = PantherResult<()>> + Send + 'a>>;

    fn send_file<'a>(
        &'a self,
        chat_id: &'a str,
        path: &'a PathBuf,
        kind: FileKind,
    ) -> Pin<Box<dyn Future<Output = PantherResult<()>> + Send + 'a>>;

    fn supports_live_status(&self) -> bool {
        false
    }

    fn send_status<'a>(
        &'a self,
        _chat_id: &'a str,
        _text: &'a str,
    ) -> Pin<Box<dyn Future<Output = PantherResult<Option<u64>>> + Send + 'a>> {
        Box::pin(async move { Ok(None) })
    }

    fn edit_status<'a>(
        &'a self,
        _chat_id: &'a str,
        _message_id: u64,
        _text: &'a str,
    ) -> Pin<Box<dyn Future<Output = PantherResult<()>> + Send + 'a>> {
        Box::pin(async move { Ok(()) })
    }

    fn delete_status<'a>(
        &'a self,
        _chat_id: &'a str,
        _message_id: u64,
    ) -> Pin<Box<dyn Future<Output = PantherResult<()>> + Send + 'a>> {
        Box::pin(async move { Ok(()) })
    }
}
