use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("missing GitHub token: send 'Authorization: Bearer <token>' or set GITHUB_TOKEN")]
    MissingToken,
    #[error("invalid request: {0}")]
    BadRequest(String),
    #[error("GitHub rejected the token")]
    Unauthorized,
    #[error("GitHub denied access: {0}")]
    Forbidden(String),
    #[error("GitHub resource not found: {0}")]
    NotFound(String),
    #[error("GitHub rate limit reached (resets at unix time {reset:?})")]
    RateLimited { reset: Option<i64> },
    #[error("GitHub returned {status}: {message}")]
    Upstream { status: u16, message: String },
    #[error("network error while calling GitHub: {0}")]
    Network(#[from] reqwest::Error),
}

impl AppError {
    fn status_and_code(&self) -> (StatusCode, &'static str) {
        match self {
            Self::MissingToken => (StatusCode::UNAUTHORIZED, "missing_token"),
            Self::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "invalid_token"),
            Self::Forbidden(_) => (StatusCode::FORBIDDEN, "forbidden"),
            Self::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            Self::RateLimited { .. } => (StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
            Self::Upstream { .. } => (StatusCode::BAD_GATEWAY, "upstream_error"),
            Self::Network(_) => (StatusCode::BAD_GATEWAY, "network_error"),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code) = self.status_and_code();
        if status.is_server_error() {
            tracing::error!(error = %self, "request failed");
        }
        let body = Json(json!({ "error": { "code": code, "message": self.to_string() } }));
        (status, body).into_response()
    }
}
