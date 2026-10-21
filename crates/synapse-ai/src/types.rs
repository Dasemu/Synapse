use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
        }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ChatRequest {
    pub system: Option<String>,
    pub messages: Vec<Message>,
    /// Overrides `ProviderConfig::max_tokens`.
    pub max_tokens: Option<u32>,
    /// Ask the provider for JSON output where it has a native mode (OpenAI, Gemini). Anthropic
    /// relies on the prompt; callers must still parse defensively.
    pub json: bool,
}

impl ChatRequest {
    pub fn new(system: impl Into<String>, user: impl Into<String>) -> Self {
        Self {
            system: Some(system.into()),
            messages: vec![Message::user(user)],
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl Usage {
    pub fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

impl std::ops::AddAssign for Usage {
    fn add_assign(&mut self, o: Usage) {
        self.input_tokens += o.input_tokens;
        self.output_tokens += o.output_tokens;
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChatResponse {
    pub text: String,
    pub usage: Usage,
    /// Provider-specific stop reason (`end_turn`, `stop`, `max_tokens`…).
    pub stop_reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EmbedResponse {
    pub vectors: Vec<Vec<f32>>,
    pub usage: Usage,
}

/// Receives text deltas as they stream in.
pub type TokenSink<'a> = &'a mut (dyn FnMut(&str) + Send);
