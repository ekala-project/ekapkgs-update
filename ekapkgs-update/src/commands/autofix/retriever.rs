//! RAG retriever for the autofix pipeline.
//!
//! Stores embeddings of error contexts from past autofix attempts and
//! retrieves the most similar successful fixes as few-shot examples for
//! the LLM prompt.
//!
//! Embeddings are generated via the LLM server's `/v1/embeddings` endpoint
//! and stored as JSON float arrays in SQLite. Cosine similarity is computed
//! in Rust at query time — this is efficient for the small dataset sizes
//! expected (hundreds to low thousands of entries).

use anyhow::{Context, Result};
use chrono::Utc;
use sqlx::Row;
use tracing::{debug, info};

use crate::database::Database;
use crate::llm::LlmClient;

/// A retrieved similar fix to use as a few-shot example.
#[derive(Debug, Clone)]
pub struct SimilarFix {
    /// Short description of the error that was fixed.
    pub error_summary: String,
    /// The JSON changes that successfully fixed the build.
    pub fix_json: String,
    /// Cosine similarity score (0.0 to 1.0).
    pub similarity: f32,
}

/// Maximum number of similar fixes to include in the prompt.
/// Kept low to stay within the small model's context window.
const MAX_EXAMPLES: usize = 2;

/// Minimum similarity threshold to include an example.
const MIN_SIMILARITY: f32 = 0.5;

/// Build a short text summary of an error context for embedding.
///
/// This is the string that gets embedded — kept compact so embeddings
/// are meaningful and comparable.
pub fn build_error_summary(
    attr_path: &str,
    error_type: &str,
    build_log_tail: Option<&str>,
) -> String {
    let mut summary = format!("Nix build error: {error_type} in {attr_path}");

    if let Some(log) = build_log_tail {
        // Take just the last few meaningful lines for the embedding
        let tail: String = log
            .lines()
            .rev()
            .take(10)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        summary.push('\n');
        summary.push_str(&tail);
    }

    summary
}

/// Store an embedding for a completed autofix attempt.
///
/// Called after each attempt (successful or not) to build up the knowledge
/// base. Returns `Ok(false)` if the embedding server is unavailable (non-fatal).
pub async fn store_embedding(
    db: &Database,
    llm: &LlmClient,
    attempt_id: i64,
    error_type: &str,
    error_summary: &str,
    fix_json: Option<&str>,
    build_success: bool,
) -> Result<bool> {
    let Some(embedding) = llm.embed(error_summary).await? else {
        debug!("Embedding server unavailable, skipping storage");
        return Ok(false);
    };

    let embedding_json = serde_json::to_string(&embedding)
        .context("serialize embedding")?;
    let now = Utc::now().to_rfc3339();

    sqlx::query(
        "INSERT INTO autofix_embeddings
            (attempt_id, error_type, error_summary, embedding, fix_json, build_success, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(attempt_id)
    .bind(error_type)
    .bind(error_summary)
    .bind(&embedding_json)
    .bind(fix_json)
    .bind(build_success)
    .bind(&now)
    .execute(db.pool())
    .await
    .context("store autofix embedding")?;

    debug!("Stored embedding for attempt {attempt_id} (dim={})", embedding.len());
    Ok(true)
}

/// Retrieve the most similar successful fixes for a given error context.
///
/// Returns up to [`MAX_EXAMPLES`] examples above [`MIN_SIMILARITY`] threshold,
/// ordered by similarity descending.
///
/// Returns an empty vec if embeddings are unavailable (non-fatal).
pub async fn retrieve_similar_fixes(
    db: &Database,
    llm: &LlmClient,
    error_type: &str,
    error_summary: &str,
) -> Result<Vec<SimilarFix>> {
    // Generate embedding for the query
    let Some(query_embedding) = llm.embed(error_summary).await? else {
        debug!("Embedding server unavailable, skipping retrieval");
        return Ok(Vec::new());
    };

    // Load all successful fix embeddings of the same error type
    let rows = sqlx::query(
        "SELECT error_summary, embedding, fix_json
         FROM autofix_embeddings
         WHERE build_success = 1 AND fix_json IS NOT NULL AND error_type = ?",
    )
    .bind(error_type)
    .fetch_all(db.pool())
    .await
    .context("query autofix embeddings")?;

    if rows.is_empty() {
        debug!("No successful fix embeddings found for error type '{error_type}'");
        return Ok(Vec::new());
    }

    // Compute cosine similarity for each candidate
    let mut candidates: Vec<SimilarFix> = Vec::new();

    for row in &rows {
        let embedding_json: String = row.get("embedding");
        let stored_embedding: Vec<f32> = match serde_json::from_str(&embedding_json) {
            Ok(e) => e,
            Err(_) => continue,
        };

        let similarity = cosine_similarity(&query_embedding, &stored_embedding);

        if similarity >= MIN_SIMILARITY {
            candidates.push(SimilarFix {
                error_summary: row.get("error_summary"),
                fix_json: row.get("fix_json"),
                similarity,
            });
        }
    }

    // Sort by similarity descending, take top N
    candidates.sort_by(|a, b| b.similarity.partial_cmp(&a.similarity).unwrap_or(std::cmp::Ordering::Equal));
    candidates.truncate(MAX_EXAMPLES);

    if !candidates.is_empty() {
        info!(
            "Retrieved {} similar fix(es) (best similarity: {:.3})",
            candidates.len(),
            candidates[0].similarity
        );
    }

    Ok(candidates)
}

/// Cosine similarity between two vectors.
///
/// Returns 0.0 if either vector has zero magnitude or they differ in length.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let mag_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let mag_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if mag_a == 0.0 || mag_b == 0.0 {
        return 0.0;
    }

    dot / (mag_a * mag_b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - (-1.0)).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_different_lengths() {
        let a = vec![1.0, 2.0];
        let b = vec![1.0, 2.0, 3.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn test_cosine_similarity_zero_vector() {
        let a = vec![0.0, 0.0];
        let b = vec![1.0, 2.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn test_build_error_summary_basic() {
        let summary = build_error_summary("pkgs.hello", "build_error", None);
        assert!(summary.contains("build_error"));
        assert!(summary.contains("pkgs.hello"));
    }

    #[test]
    fn test_build_error_summary_with_log() {
        let log = "line1\nline2\nline3\nerror: missing dependency foo";
        let summary = build_error_summary("pkgs.hello", "build_error", Some(log));
        assert!(summary.contains("missing dependency foo"));
    }
}
