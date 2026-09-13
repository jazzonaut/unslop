//! Rewriting through an OpenAI-compatible API.
//!
//! This is the one path that sends clipboard text off the machine, so it is
//! opt-in, it is announced in the popup, and the fact placeholders still apply:
//! prices, addresses and dates are already substituted out before anything is
//! sent. The prose around them is not.
//!
//! Any endpoint speaking the OpenAI chat schema works, which covers OpenRouter,
//! OpenAI itself, and anything self-hosted.

use crate::config::Remote;

pub fn rewrite(
    remote: &Remote,
    system: &str,
    text: &str,
    temperature: f32,
    max_tokens: u32,
) -> Result<String, String> {
    let key = remote
        .key()
        .ok_or("no API key: set remote.api_key or OPENROUTER_API_KEY")?;

    let request = serde_json::json!({
        "model": remote.model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": text},
        ],
        "temperature": temperature,
        "max_tokens": max_tokens,
    });

    let url = format!("{}/chat/completions", remote.base_url.trim_end_matches('/'));
    let mut response = ureq::post(&url)
        .config()
        .timeout_global(Some(crate::model::CALL_TIMEOUT))
        .build()
        .header("Authorization", &format!("Bearer {key}"))
        // OpenRouter attributes requests by these, and they are harmless
        // anywhere else.
        .header("HTTP-Referer", "https://github.com/unslop")
        .header("X-Title", "Unslop")
        .send_json(&request)
        .map_err(|err| format!("{} did not answer: {err}", remote.base_url))?;

    let body: serde_json::Value = response
        .body_mut()
        .read_json()
        .map_err(|err| format!("unreadable reply: {err}"))?;

    // An error here is a well-formed reply carrying a refusal or a billing
    // problem, which is worth passing on verbatim.
    if let Some(message) = body["error"]["message"].as_str() {
        return Err(message.to_owned());
    }

    body["choices"][0]["message"]["content"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "the reply contained no content".to_owned())
}
