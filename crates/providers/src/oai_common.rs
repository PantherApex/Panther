use serde::{Deserialize, Serialize};
use serde_json::Value;
use shared::errors::{PantherError, PantherResult};
use shared::types::{LLMProvider, LLMRequest, LLMResponse, ToolCall};

#[derive(Serialize)]
pub struct OAIFunction<'a> {
    pub name: &'a str,
    pub description: &'a str,
    pub parameters: &'a Value,
}

#[derive(Serialize)]
pub struct OAITool<'a> {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: OAIFunction<'a>,
}

#[derive(Serialize)]
pub struct OAIToolCallFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Serialize)]
pub struct OAIToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: OAIToolCallFunction,
}

#[derive(Serialize)]
pub struct OAIMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<OAIToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Serialize)]
pub struct OAIRequest<'a> {
    pub model: &'a str,
    pub messages: Vec<OAIMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<OAITool<'a>>>,
}

#[derive(Deserialize)]
pub struct OAIRespFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Deserialize)]
pub struct OAIRespToolCall {
    pub id: String,
    pub function: OAIRespFunction,
}

#[derive(Deserialize)]
pub struct OAIRespMessage {
    pub content: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<OAIRespToolCall>,
}

#[derive(Deserialize)]
pub struct OAIChoice {
    pub message: OAIRespMessage,
    pub finish_reason: Option<String>,
}

#[derive(Deserialize)]
pub struct OAIResponse {
    pub choices: Vec<OAIChoice>,
    pub model: String,
}

pub fn build_oai_messages(request: &LLMRequest) -> Vec<OAIMessage> {
    request.messages.iter().map(|m| {
        let tool_calls = m.tool_calls.as_ref().map(|tcs| {
            tcs.iter().map(|tc| OAIToolCall {
                id: tc.call_id.clone(),
                kind: "function",
                function: OAIToolCallFunction {
                    name: tc.name.clone(),
                    arguments: tc.arguments.to_string(),
                },
            }).collect()
        });
        OAIMessage {
            role: m.role.clone(),
            content: m.content.clone(),
            tool_calls,
            tool_call_id: m.tool_call_id.clone(),
        }
    }).collect()
}

pub fn build_oai_tools(request: &LLMRequest) -> Option<Vec<OAITool<'_>>> {
    request.tools.as_ref().map(|defs| {
        defs.iter().map(|d| OAITool {
            kind: "function",
            function: OAIFunction {
                name: &d.name,
                description: &d.description,
                parameters: &d.parameters,
            },
        }).collect()
    })
}

pub fn parse_oai_response(parsed: OAIResponse, provider: LLMProvider) -> PantherResult<LLMResponse> {
    let choice = parsed
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| PantherError::LLMError("Provider returned empty choices".into()))?;

    let tool_calls: Vec<ToolCall> = choice.message.tool_calls
        .into_iter()
        .map(|tc| {
            let arguments: Value = serde_json::from_str(&tc.function.arguments)
                .unwrap_or(Value::Object(serde_json::Map::new()));
            ToolCall {
                call_id: tc.id,
                name: tc.function.name,
                arguments,
            }
        })
        .collect();

    Ok(LLMResponse {
        content: choice.message.content,
        model: parsed.model,
        provider,
        tool_calls: if tool_calls.is_empty() { None } else { Some(tool_calls) },
        finish_reason: choice.finish_reason,
    })
}
