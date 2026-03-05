use async_trait::async_trait;
use shared::errors::PantherResult;
use shared::types::{LLMRequest, LLMResponse};

#[async_trait]
pub trait ProviderInterface: Send + Sync {
    async fn chat(&self, request: LLMRequest) -> PantherResult<LLMResponse>;
}
