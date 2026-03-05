use shared::errors::PantherResult;
use shared::types::{LLMProvider, LLMRequest, LLMResponse};
use crate::anthropic::AnthropicProvider;
use crate::cohere::CohereProvider;
use crate::deepseek::DeepSeekProvider;
use crate::gemini::GeminiProvider;
use crate::groq::GroqProvider;
use crate::interface::ProviderInterface;
use crate::mistral::MistralProvider;
use crate::ollama::OllamaProvider;
use crate::openai::OpenAIProvider;
use crate::openrouter::OpenRouterProvider;
use crate::perplexity::PerplexityProvider;
use crate::together::TogetherAIProvider;
use crate::xai::XAIProvider;

#[derive(Clone)]
pub struct ProviderRouter {
    pub active: LLMProvider,
    ollama: OllamaProvider,
    openai: OpenAIProvider,
    anthropic: AnthropicProvider,
    openrouter: OpenRouterProvider,
    gemini: GeminiProvider,
    groq: GroqProvider,
    mistral: MistralProvider,
    deepseek: DeepSeekProvider,
    xai: XAIProvider,
    together: TogetherAIProvider,
    perplexity: PerplexityProvider,
    cohere: CohereProvider,
    ollama_model: String,
    cloud_model: String,
}

impl ProviderRouter {
    pub fn new(
        active: LLMProvider,
        ollama_host: String,
        openai_key: String,
        anthropic_key: String,
        openrouter_key: String,
        gemini_key: String,
        groq_key: String,
        mistral_key: String,
        deepseek_key: String,
        xai_key: String,
        together_key: String,
        perplexity_key: String,
        cohere_key: String,
        ollama_model: String,
        cloud_model: String,
    ) -> Self {
        Self {
            active,
            ollama: OllamaProvider::new(ollama_host),
            openai: OpenAIProvider::new(openai_key),
            anthropic: AnthropicProvider::new(anthropic_key),
            openrouter: OpenRouterProvider::new(openrouter_key),
            gemini: GeminiProvider::new(gemini_key),
            groq: GroqProvider::new(groq_key),
            mistral: MistralProvider::new(mistral_key),
            deepseek: DeepSeekProvider::new(deepseek_key),
            xai: XAIProvider::new(xai_key),
            together: TogetherAIProvider::new(together_key),
            perplexity: PerplexityProvider::new(perplexity_key),
            cohere: CohereProvider::new(cohere_key),
            ollama_model,
            cloud_model,
        }
    }

    pub fn switch(&mut self, provider: LLMProvider) {
        self.active = provider;
    }

    pub fn active_provider(&self) -> &LLMProvider {
        &self.active
    }

    pub fn active_model(&self) -> String {
        match self.active {
            LLMProvider::Ollama => self.ollama_model.clone(),
            _ => self.cloud_model.clone(),
        }
    }

    pub async fn chat(&self, request: LLMRequest) -> PantherResult<LLMResponse> {
        match self.active {
            LLMProvider::Ollama => self.ollama.chat(request).await,
            LLMProvider::OpenAI => self.openai.chat(request).await,
            LLMProvider::Anthropic => self.anthropic.chat(request).await,
            LLMProvider::OpenRouter => self.openrouter.chat(request).await,
            LLMProvider::Gemini => self.gemini.chat(request).await,
            LLMProvider::Groq => self.groq.chat(request).await,
            LLMProvider::Mistral => self.mistral.chat(request).await,
            LLMProvider::DeepSeek => self.deepseek.chat(request).await,
            LLMProvider::XAI => self.xai.chat(request).await,
            LLMProvider::TogetherAI => self.together.chat(request).await,
            LLMProvider::Perplexity => self.perplexity.chat(request).await,
            LLMProvider::Cohere => self.cohere.chat(request).await,
        }
    }
}
