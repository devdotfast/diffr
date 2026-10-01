//! Each model API's wire format: where a request goes, how it is
//! authenticated, and where the answer's text is.
use serde::de::DeserializeOwned;
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

    /// Every provider constrains the answer with the same schema.
    /// Only Gemini gets a temperature: current reasoning models reject one.
    /// OpenAI gets no length limit either, since compatible servers name it
    /// differently; Anthropic's limit also covers thinking, so it has a floor.
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
                    "responseJsonSchema": summaries_schema(),
                },
            }),
            Self::OpenAi => json!({
                "model": model,
                "messages": [
                    {"role": "system", "content": system},
                    {"role": "user", "content": user},
                ],
                "response_format": {
                    "type": "json_schema",
                    "json_schema": {"name": "summaries", "strict": true, "schema": summaries_schema()},
                },
            }),
            Self::Anthropic => json!({
                "model": model,
                "max_tokens": max_tokens.max(4096),
                "system": system,
                "messages": [{"role": "user", "content": user}],
                "output_config": {"format": {"type": "json_schema", "schema": summaries_schema()}},
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

/// `{"summaries": [{id, summary, pseudocode}]}`, every field required: OpenAI
/// and Anthropic take only an object at the root.
fn summaries_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "summaries": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "integer"},
                        "summary": {"type": "string"},
                        "pseudocode": {"type": "string"},
                    },
                    "required": ["id", "summary", "pseudocode"],
                    "additionalProperties": false,
                },
            },
        },
        "required": ["summaries"],
        "additionalProperties": false,
    })
}

/// The first JSON array in an answer that holds items, ignoring any prose,
/// fence or reasoning around it, and the `summaries` object wrapping it;
/// an empty array only when none does.
pub fn answers<T: DeserializeOwned>(text: &str) -> Option<Vec<T>> {
    let mut parsed = text.match_indices('[').filter_map(|(start, _)| {
        serde_json::Deserializer::from_str(&text[start..])
            .into_iter::<Vec<T>>()
            .next()?
            .ok()
    });
    let first = parsed.next()?;
    if !first.is_empty() {
        return Some(first);
    }
    Some(parsed.find(|items| !items.is_empty()).unwrap_or(first))
}
