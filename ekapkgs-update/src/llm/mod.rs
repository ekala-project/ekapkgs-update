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

use self::types::{ChatCompletionRequest, EmbeddingRequest, EmbeddingResponse};
pub use self::types::{ChatCompletionResponse, ChatMessage};
use crate::config::LlmConfig;

/// Default model name.
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
    /// Create a client from a config file section, falling back to env vars
    /// and built-in defaults.
    ///
    /// Returns an error if no base URL is configured anywhere.
    pub fn from_config(llm_config: &LlmConfig) -> anyhow::Result<Self> {
        let base_url = llm_config.resolve_base_url().context(
            "LLM base URL is required: set llm.base_url in config file or EKAPKGS_LLM_BASE_URL \
             env var",
        )?;

        let model = llm_config
            .resolve_model()
            .unwrap_or_else(|| DEFAULT_MODEL.to_owned());

        let max_tokens = llm_config
            .resolve_max_tokens()
            .unwrap_or(DEFAULT_MAX_TOKENS);

        let temperature = llm_config
            .resolve_temperature()
            .unwrap_or(DEFAULT_TEMPERATURE);

        let timeout = llm_config
            .resolve_timeout_secs()
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

    /// Create a client from environment variables only (no config file).
    ///
    /// Convenience wrapper for contexts where no config file is loaded.
    #[allow(dead_code)]
    pub fn from_env() -> anyhow::Result<Self> {
        Self::from_config(&LlmConfig::default())
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
                warn!(
                    "LLM request failed, retrying in {}s (attempt {}/{})",
                    delay.as_secs(),
                    attempt + 1,
                    MAX_RETRIES + 1
                );
                tokio::time::sleep(delay).await;
            }

            debug!(
                "Sending chat completion request to {} (attempt {})",
                url,
                attempt + 1
            );

            let result = self.client.post(&url).json(&request_body).send().await;

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
