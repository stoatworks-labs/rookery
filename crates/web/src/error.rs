use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

pub enum ApiError {
    NotFound(String),
    BadRequest(String),
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        ApiError::Internal(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::NotFound(m) => (StatusCode::NOT_FOUND, m),
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            ApiError::Internal(e) => {
                tracing::error!("internal error: {e:#}");
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
        };
        (status, axum::Json(serde_json::json!({ "error": message }))).into_response()
    }
}

pub fn parse_instance_id(raw: &str) -> Result<rookery_core::InstanceId, ApiError> {
    raw.parse()
        .map_err(|_| ApiError::BadRequest(format!("invalid instance id: {raw}")))
}
