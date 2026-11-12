//! Error responses: HTML pages for browsers, JSON for `/api/*`.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use crate::templates::ErrorTemplate;

/// Error returned by HTML handlers.
#[derive(Debug)]
pub enum AppError {
    /// 404 with a human explanation.
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

    fn log(&self) {
        if let Self::Internal(msg) = self {
            tracing::error!("request failed: {msg}");
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        self.log();
        let status = self.status();
        let template = ErrorTemplate {
            code: status.as_u16(),
            title: if status == StatusCode::NOT_FOUND {
                "Not found".to_owned()
            } else {
                "Something went wrong".to_owned()
            },
            message: self.public_message(),
        };
        (status, template).into_response()
    }
}

/// Error returned by JSON handlers: same semantics, JSON body.
#[derive(Debug)]
pub struct ApiError(pub AppError);

impl From<AppError> for ApiError {
    fn from(e: AppError) -> Self {
        Self(e)
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        Self(AppError::from(e))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        self.0.log();
        let status = self.0.status();
        (
            status,
            Json(
                serde_json::json!({ "error": self.0.public_message(), "status": status.as_u16() }),
            ),
        )
            .into_response()
    }
}

/// Router fallback: unknown paths render the 404 page.
pub async fn fallback() -> AppError {
    AppError::NotFound("There is no page at this address.".to_owned())
}

pub type AppResult<T> = Result<T, AppError>;
pub type ApiResult<T> = Result<T, ApiError>;
