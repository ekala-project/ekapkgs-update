//! LLM client for OpenAI-compatible chat completion APIs.
//!
//! Connects to a local or remote LLM server (e.g., Ollama, vLLM, llama.cpp)
//! that exposes an OpenAI-compatible `/v1/chat/completions` endpoint.
//!
//! Configuration is via environment variables:
//!
//! - `EKAPKGS_LLM_BASE_URL` (required) — e.g. `http://llm-server:8080`
//! - `EKAPKGS_LLM_MODEL` — model name (default: `qwen2.5-coder:3b`)
//! - `EKAPKGS_LLM_MAX_TOKENS` — max response tokens (default: 2048)
//! - `EKAPKGS_LLM_TEMPERATURE` — sampling temperature (default: 0.1)

pub mod types;

use std::time::Duration;

use anyhow::{Context, bail};
use tracing::{debug, warn};

pub use self::types::{ChatCompletionResponse, ChatMessage};
use self::types::{ChatCompletionRequest, EmbeddingRequest, EmbeddingResponse};

/// Default model name if `EKAPKGS_LLM_MODEL` is not set.
const DEFAULT_MODEL: &str = "qwen2.5-coder:3b";
/// Default maximum response tokens.
const DEFAULT_MAX_TOKENS: u32 = 2048;
/// Default sampling temperature (low for deterministic output).
const DEFAULT_TEMPERATURE: f32 = 0.1;
/// Request timeout — small models on CPU can be slow.
const DEFAULT_TIMEOUT_SECS: u64 = 300;
/// Maximum number of retries on transient network errors.
const MAX_RETRIES: u32 = 2;

/// Client for an OpenAI-compatible chat completion API.
pub struct LlmClient {
    client: reqwest::Client,
    base_url: String,
    model: String,
    max_tokens: u32,
    temperature: f32,
}

impl LlmClient {
    /// Create a client from environment variables.
    ///
    /// Returns an error if `EKAPKGS_LLM_BASE_URL` is not set.
    pub fn from_env() -> anyhow::Result<Self> {
        let base_url = std::env::var("EKAPKGS_LLM_BASE_URL")
            .context("EKAPKGS_LLM_BASE_URL environment variable is required for autofix")?;

        let model = std::env::var("EKAPKGS_LLM_MODEL")
            .unwrap_or_else(|_| DEFAULT_MODEL.to_owned());

        let max_tokens = std::env::var("EKAPKGS_LLM_MAX_TOKENS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_MAX_TOKENS);

        let temperature = std::env::var("EKAPKGS_LLM_TEMPERATURE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_TEMPERATURE);

        let timeout = std::env::var("EKAPKGS_LLM_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_TIMEOUT_SECS);

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout))
            .build()
            .context("Failed to build HTTP client")?;

        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_owned(),
            model,
            max_tokens,
            temperature,
        })
    }

    /// Send a chat completion request and return the response.
    ///
    /// Retries up to [`MAX_RETRIES`] times on transient network errors with
    /// exponential backoff (2s, 4s).
    pub async fn chat_completion(
        &self,
        messages: Vec<ChatMessage>,
    ) -> anyhow::Result<ChatCompletionResponse> {
        let url = format!("{}/v1/chat/completions", self.base_url);
        let request_body = ChatCompletionRequest {
            model: self.model.clone(),
            messages,
            temperature: Some(self.temperature),
            max_tokens: Some(self.max_tokens),
        };

        let mut last_error = None;

        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                let delay = Duration::from_secs(2u64.pow(attempt));
                warn!("LLM request failed, retrying in {}s (attempt {}/{})", delay.as_secs(), attempt + 1, MAX_RETRIES + 1);
                tokio::time::sleep(delay).await;
            }

            debug!("Sending chat completion request to {} (attempt {})", url, attempt + 1);

            let result = self
                .client
                .post(&url)
                .json(&request_body)
                .send()
                .await;

            let response = match result {
                Ok(r) => r,
                Err(e) if e.is_timeout() || e.is_connect() => {
                    last_error = Some(format!("Network error: {e}"));
                    continue;
                },
                Err(e) => bail!("LLM request failed: {e}"),
            };

            let status = response.status();
            if status == reqwest::StatusCode::SERVICE_UNAVAILABLE
                || status == reqwest::StatusCode::TOO_MANY_REQUESTS
                || status == reqwest::StatusCode::INTERNAL_SERVER_ERROR
            {
                let body = response.text().await.unwrap_or_default();
                last_error = Some(format!("HTTP {status}: {body}"));
                continue;
            }

            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                bail!("LLM API error (HTTP {status}): {body}");
            }

            let parsed: ChatCompletionResponse = response
                .json()
                .await
                .context("Failed to parse LLM response JSON")?;

            debug!(
                "LLM response received: {} choices, usage: {:?}",
                parsed.choices.len(),
                parsed.usage
            );

            return Ok(parsed);
        }

        bail!(
            "LLM request failed after {} attempts: {}",
            MAX_RETRIES + 1,
            last_error.unwrap_or_else(|| "unknown error".to_owned())
        )
    }

    /// Generate an embedding vector for the given text.
    ///
    /// Uses the `/v1/embeddings` endpoint. If the server doesn't support
    /// embeddings, returns `None` rather than failing the whole pipeline.
    pub async fn embed(&self, text: &str) -> anyhow::Result<Option<Vec<f32>>> {
        let url = format!("{}/v1/embeddings", self.base_url);
        let request_body = EmbeddingRequest {
            model: self.model.clone(),
            input: text.to_owned(),
        };

        let result = self.client.post(&url).json(&request_body).send().await;

        let response = match result {
            Ok(r) => r,
            Err(e) => {
                debug!("Embedding request failed (non-fatal): {e}");
                return Ok(None);
            },
        };

        if !response.status().is_success() {
            debug!(
                "Embedding endpoint returned HTTP {} (non-fatal)",
                response.status()
            );
            return Ok(None);
        }

        let parsed: EmbeddingResponse = match response.json().await {
            Ok(p) => p,
            Err(e) => {
                debug!("Failed to parse embedding response (non-fatal): {e}");
                return Ok(None);
            },
        };

        Ok(parsed.data.into_iter().next().map(|d| d.embedding))
    }

    /// The model name this client is configured to use.
    #[allow(dead_code)]
    pub fn model(&self) -> &str {
        &self.model
    }
}
