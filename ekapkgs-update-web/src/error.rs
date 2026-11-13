//! API error responses: `{"error": "<message>", "status": <code>}`.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// Error returned by API handlers.
#[derive(Debug)]
pub enum AppError {
    /// 404 with a human explanation (shown by the frontend).
    NotFound(String),
    /// 500, typically a database error. The message is logged, not shown.
    Internal(String),
}

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        Self::Internal(e.to_string())
    }
}

impl AppError {
    fn status(&self) -> StatusCode {
        match self {
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn public_message(&self) -> String {
        match self {
            Self::NotFound(msg) => msg.clone(),
            Self::Internal(_) => "The dashboard could not read the database. The error has been \
                                  logged; check the server logs for details."
                .to_owned(),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        if let Self::Internal(msg) = &self {
            tracing::error!("request failed: {msg}");
        }
        let status = self.status();
        (
            status,
            Json(serde_json::json!({ "error": self.public_message(), "status": status.as_u16() })),
        )
            .into_response()
    }
}

pub type ApiResult<T> = Result<T, AppError>;
