//! Each model API's wire format: where a request goes, how it is
//! authenticated, and where the answer's text is.
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Gemini,
    OpenAi,
    Anthropic,
}

impl Provider {
    pub fn default_endpoint(self) -> &'static str {
        match self {
            Self::Gemini => "https://generativelanguage.googleapis.com",
            Self::OpenAi => "https://api.openai.com/v1",
            Self::Anthropic => "https://api.anthropic.com",
        }
    }

    /// The environment variables read, in order, when `api_key` is unset.
    pub fn key_variables(self) -> &'static [&'static str] {
        match self {
            Self::Gemini => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
            Self::OpenAi => &["OPENAI_API_KEY"],
            Self::Anthropic => &["ANTHROPIC_API_KEY"],
        }
    }

    /// OpenAI-compatible servers at a custom endpoint, such as Ollama,
    /// often take no key.
    pub fn key_optional(self, custom_endpoint: bool) -> bool {
        self == Self::OpenAi && custom_endpoint
    }

    pub fn url(self, endpoint: &str, model: &str) -> String {
        let endpoint = endpoint.trim_end_matches('/');
        match self {
            Self::Gemini => format!("{endpoint}/v1beta/models/{model}:generateContent"),
            Self::OpenAi => format!("{endpoint}/chat/completions"),
            Self::Anthropic => format!("{endpoint}/v1/messages"),
        }
    }

    pub fn headers(self, key: Option<&str>) -> Vec<(&'static str, String)> {
        let mut headers = Vec::new();
        if let Some(key) = key {
            headers.push(match self {
                Self::Gemini => ("x-goog-api-key", key.to_owned()),
                Self::OpenAi => ("authorization", format!("Bearer {key}")),
                Self::Anthropic => ("x-api-key", key.to_owned()),
            });
        }
        if self == Self::Anthropic {
            headers.push(("anthropic-version", "2023-06-01".to_owned()));
        }
        headers
    }

    /// Only Gemini constrains the answer with a schema; the others follow
    /// the system prompt, and [`answer_json`] drops what they wrap it in.
    /// OpenAI gets no sampling or length options: reasoning models reject
    /// them and compatible servers name them differently.
    pub fn body(self, model: &str, system: &str, user: &str, max_tokens: usize) -> Value {
        match self {
            Self::Gemini => json!({
                "systemInstruction": {"parts": [{"text": system}]},
                "contents": [{"role": "user", "parts": [{"text": user}]}],
                "generationConfig": {
                    "temperature": 0,
                    "maxOutputTokens": max_tokens,
                    "thinkingConfig": {"thinkingBudget": 0},
                    "responseMimeType": "application/json",
                    "responseSchema": {
                        "type": "ARRAY",
                        "items": {
                            "type": "OBJECT",
                            "properties": {
                                "id": {"type": "INTEGER"},
                                "summary": {"type": "STRING"},
                                "pseudocode": {"type": "STRING"},
                            },
                            "required": ["id", "pseudocode"],
                        },
                    },
                },
            }),
            Self::OpenAi => json!({
                "model": model,
                "messages": [
                    {"role": "system", "content": system},
                    {"role": "user", "content": user},
                ],
            }),
            Self::Anthropic => json!({
                "model": model,
                "max_tokens": max_tokens,
                "temperature": 0,
                "system": system,
                "messages": [{"role": "user", "content": user}],
            }),
        }
    }

    pub fn text(self, response: &Value) -> Option<&str> {
        match self {
            Self::Gemini => response["candidates"][0]["content"]["parts"]
                .as_array()?
                .last()?["text"]
                .as_str(),
            Self::OpenAi => response["choices"][0]["message"]["content"].as_str(),
            Self::Anthropic => response["content"]
                .as_array()?
                .iter()
                .rev()
                .find(|block| block["type"] == "text")?["text"]
                .as_str(),
        }
    }
}

/// The JSON array in an answer, without any prose or code fence around it.
pub fn answer_json(text: &str) -> &str {
    match (text.find('['), text.rfind(']')) {
        (Some(start), Some(end)) if start < end => &text[start..=end],
        _ => text,
    }
}
