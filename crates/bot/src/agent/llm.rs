//! Minimal client for an `OpenAI`-compatible chat-completions endpoint (Ollama
//! Cloud). Only the request/response shape the agent loop needs is modelled;
//! everything else in the provider's schema is ignored on deserialize.

use std::time::{Duration, Instant};

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

    /// An assistant turn the loop wrote rather than the model — the record of a
    /// turn that ended without the model producing a final message of its own.
    pub(crate) fn assistant(content: impl Into<String>) -> Self {
        Self::text(Role::Assistant, content)
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

/// The single place generated tool specs cross into the bot's wire type. Every
/// `<Tool>::spec()` flows through here, so the LLM client's own types stay
/// unaware of `grizzly-agent`. `ToolSpec`'s `name`/`description` are
/// `Cow<'static, str>` — borrowed for every generated tool, owned for any tool
/// built at runtime — and convert into `String` either way.
impl From<grizzly_agent::ToolSpec> for ToolDef {
    fn from(spec: grizzly_agent::ToolSpec) -> Self {
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
    #[serde(default)]
    usage: Option<TokenUsage>,
    /// The provider's echo of which model actually answered, which can differ
    /// from the tag we asked for (aliases, routing).
    #[serde(default)]
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ChatMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

/// Token accounting for one model call, as the provider reported it. Every count
/// is optional because providers differ in what they return, and a count that
/// wasn't sent must read as *unknown* rather than as zero.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct TokenUsage {
    #[serde(default, rename = "prompt_tokens")]
    pub(crate) prompt: Option<u32>,
    #[serde(default, rename = "completion_tokens")]
    pub(crate) completion: Option<u32>,
    #[serde(default, rename = "total_tokens")]
    pub(crate) total: Option<u32>,
    #[serde(default, rename = "prompt_tokens_details")]
    pub(crate) prompt_details: Option<PromptTokenDetails>,
}

/// The provider's breakdown of what the prompt tokens were made of. Only the
/// cache-hit count is modelled — it is the one that bills at a different rate.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct PromptTokenDetails {
    #[serde(default, rename = "cached_tokens")]
    pub(crate) cached: Option<u32>,
}

impl TokenUsage {
    /// Prompt tokens the provider served from its prefix cache, when it said so.
    pub(crate) fn cached_prompt(&self) -> Option<u32> {
        self.prompt_details
            .as_ref()
            .and_then(|details| details.cached)
    }

    /// Prompt tokens that were *not* cache hits — the count the full input rate
    /// applies to.
    ///
    /// `prompt_tokens` is inclusive of the cache hits, so anything that prices
    /// the two counts side by side has to subtract here or bill the cached
    /// tokens twice.
    pub(crate) fn uncached_prompt(&self) -> Option<u32> {
        self.prompt
            .map(|prompt| prompt.saturating_sub(self.cached_prompt().unwrap_or(0)))
    }
}

/// One model call's full result: the assistant's message plus everything
/// measurable about the call that produced it.
#[derive(Clone, Debug)]
pub(crate) struct Completion {
    pub(crate) message: ChatMessage,
    /// Whichever model answered — the provider's own echo when it sends one,
    /// else the configured tag.
    pub(crate) model: String,
    pub(crate) usage: Option<TokenUsage>,
    pub(crate) finish_reason: Option<String>,
    /// Wall-clock time for the whole round trip, request sent through body parsed.
    pub(crate) latency: Duration,
}

/// Send one chat-completion turn and return the assistant message with the call's
/// usage, finish reason, and measured latency. Advertises `tools` (with
/// `tool_choice: auto`) when any are supplied.
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
) -> Result<Completion> {
    let url = format!("{}/chat/completions", cfg.base_url.trim_end_matches('/'));
    let request = ChatRequest {
        model: &cfg.model,
        messages,
        tools,
        tool_choice: (!tools.is_empty()).then_some("auto"),
    };

    let started = Instant::now();
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
    let latency = started.elapsed();

    let ChatResponse {
        choices,
        usage,
        model,
    } = parsed;
    let choice = choices
        .into_iter()
        .next()
        .context("chat-completions reply contained no choices")?;
    Ok(Completion {
        message: choice.message,
        model: model.unwrap_or_else(|| cfg.model.clone()),
        usage,
        finish_reason: choice.finish_reason,
        latency,
    })
}

#[cfg(test)]
#[path = "tests/llm.rs"]
mod tests;
