//! The API error envelope: `{"error": {"code": "...", "message": "..."}}`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use bz_core::CoreError;
use serde_json::json;

/// An HTTP error rendered as the documented envelope.
#[derive(Debug, Clone)]
pub struct ApiErr {
    pub status: StatusCode,
    pub code: String,
    pub message: String,
}

impl ApiErr {
    pub fn new(status: StatusCode, code: &str, message: impl Into<String>) -> ApiErr {
        ApiErr {
            status,
            code: code.to_string(),
            message: message.into(),
        }
    }
    pub fn bad_request(message: impl Into<String>) -> ApiErr {
        ApiErr::new(StatusCode::BAD_REQUEST, "invalid_input", message)
    }
    pub fn not_found(message: impl Into<String>) -> ApiErr {
        ApiErr::new(StatusCode::NOT_FOUND, "not_found", message)
    }
    pub fn forbidden(message: impl Into<String>) -> ApiErr {
        ApiErr::new(StatusCode::FORBIDDEN, "forbidden", message)
    }
    pub fn unauthorized(message: impl Into<String>) -> ApiErr {
        ApiErr::new(StatusCode::UNAUTHORIZED, "unauthorized", message)
    }
    pub fn internal(message: impl Into<String>) -> ApiErr {
        ApiErr::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
    }
}

impl From<CoreError> for ApiErr {
    fn from(e: CoreError) -> ApiErr {
        ApiErr {
            status: StatusCode::from_u16(e.http_status())
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            code: e.code().to_string(),
            message: e.to_string(),
        }
    }
}

impl From<bz_net::NetError> for ApiErr {
    fn from(e: bz_net::NetError) -> ApiErr {
        ApiErr::from(crate::netprovider::net_err(e))
    }
}

impl IntoResponse for ApiErr {
    fn into_response(self) -> Response {
        let body = json!({"error": {"code": self.code, "message": self.message}});
        (self.status, Json(body)).into_response()
    }
}

/// Handler result.
pub type ApiResult<T> = Result<T, ApiErr>;
