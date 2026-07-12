//! Minimal client for an `OpenAI`-compatible chat-completions endpoint (Ollama
//! Cloud). Only the request/response shape the agent loop needs is modelled;
//! everything else in the provider's schema is ignored on deserialize.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// Per-request ceiling for a completion call. Generous because a tool-calling
/// model on a cold cloud route can take tens of seconds; the shared `reqwest`
/// client's short default (tuned for in-cluster supervisor hops) is overridden
/// here the same way the supervisor's mutating calls override it.
const COMPLETION_TIMEOUT: Duration = Duration::from_mins(2);

/// Connection settings for the agent's chat-completions endpoint.
#[derive(Clone, Debug)]
pub(crate) struct OllamaConfig {
    pub(crate) api_key: String,
    /// Base URL ending before `/chat/completions` (e.g. `https://ollama.com/v1`).
    pub(crate) base_url: String,
    pub(crate) model: String,
}

/// Who authored a message in the conversation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// One conversation message. The optional fields cover every role with a single
/// shape: `content` for text, `tool_calls` on an assistant turn that invokes
/// tools, `tool_call_id` on a tool result tying it back to its call.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ChatMessage {
    pub(crate) role: Role,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) tool_calls: Option<Vec<ToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) tool_call_id: Option<String>,
}

impl ChatMessage {
    pub(crate) fn system(content: impl Into<String>) -> Self {
        Self::text(Role::System, content)
    }

    pub(crate) fn user(content: impl Into<String>) -> Self {
        Self::text(Role::User, content)
    }

    /// A tool result fed back to the model, tied to the call it answers.
    pub(crate) fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
        }
    }

    fn text(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    /// The tool calls this (assistant) message requests, if any and non-empty.
    pub(crate) fn requested_tool_calls(&self) -> Option<&[ToolCall]> {
        self.tool_calls.as_deref().filter(|calls| !calls.is_empty())
    }
}

/// A single tool invocation requested by the model.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ToolCall {
    pub(crate) id: String,
    #[serde(rename = "type")]
    pub(crate) kind: String,
    pub(crate) function: FunctionCall,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct FunctionCall {
    pub(crate) name: String,
    /// JSON-encoded argument object, per the `OpenAI` wire format. Parsed by the
    /// dispatcher against each tool's parameter schema.
    pub(crate) arguments: String,
}

/// A tool the model may call, advertised in the request.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct ToolDef {
    #[serde(rename = "type")]
    pub(crate) kind: String,
    pub(crate) function: FunctionDef,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct FunctionDef {
    pub(crate) name: String,
    pub(crate) description: String,
    /// JSON Schema for the tool's parameters.
    pub(crate) parameters: serde_json::Value,
}

impl ToolDef {
    /// Wrap a name/description/parameter-schema triple as an `OpenAI` function tool.
    pub(crate) fn function(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        Self {
            kind: "function".to_owned(),
            function: FunctionDef {
                name: name.into(),
                description: description.into(),
                parameters,
            },
        }
    }
}

/// The single place prompt-lib's generated tool specs cross into the bot's wire
/// type. Every `<Tool>::spec()` flows through here, so the LLM client's own types
/// stay unaware of prompt-lib.
impl From<grizzly_prompt_lib::ToolSpec> for ToolDef {
    fn from(spec: grizzly_prompt_lib::ToolSpec) -> Self {
        Self::function(spec.name, spec.description, spec.parameters)
    }
}

#[derive(Clone, Debug, Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    tools: &'a [ToolDef],
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ChatMessage,
}

/// Send one chat-completion turn and return the assistant message. Advertises
/// `tools` (with `tool_choice: auto`) when any are supplied.
///
/// # Errors
///
/// Returns an error if the request fails to send, the endpoint returns a
/// non-success status, the body can't be parsed, or the response carries no
/// choices.
pub(crate) async fn send_chat_completion(
    http: &reqwest::Client,
    cfg: &OllamaConfig,
    messages: &[ChatMessage],
    tools: &[ToolDef],
) -> Result<ChatMessage> {
    let url = format!("{}/chat/completions", cfg.base_url.trim_end_matches('/'));
    let request = ChatRequest {
        model: &cfg.model,
        messages,
        tools,
        tool_choice: (!tools.is_empty()).then_some("auto"),
    };

    let response = http
        .post(&url)
        .bearer_auth(&cfg.api_key)
        .timeout(COMPLETION_TIMEOUT)
        .json(&request)
        .send()
        .await
        .with_context(|| format!("failed to reach chat-completions endpoint at {url}"))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        bail!("chat-completions endpoint returned {status}: {body}");
    }

    let parsed: ChatResponse = response
        .json()
        .await
        .with_context(|| format!("failed to parse chat-completions reply from {url}"))?;
    parsed
        .choices
        .into_iter()
        .next()
        .map(|choice| choice.message)
        .context("chat-completions reply contained no choices")
}

#[cfg(test)]
#[path = "tests/llm.rs"]
mod tests;
