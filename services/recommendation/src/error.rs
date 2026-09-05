use axum::response::IntoResponse;
use axum::{Json, http::StatusCode};
use serde_json::json;

#[derive(Debug)]
pub(crate) struct RecommendationError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl RecommendationError {
    pub(crate) fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
}

impl IntoResponse for RecommendationError {
    fn into_response(self) -> axum::response::Response {
        (
            self.status,
            Json(json!({"error": self.code, "message": self.message})),
        )
            .into_response()
    }
}
