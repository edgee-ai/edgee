#![allow(dead_code)]
//! OpenAI-compatible error response types.
//!
//! This module provides error response types that match the OpenAI API format,
//! enabling consistent error handling across all platforms.

use axum_core::response::{IntoResponse, Response};
use edgee_gateway_core::Error as CoreError;
use http::StatusCode;
use serde::Serialize;

/// OpenAI-compatible error response.
///
/// This is the top-level error response format used by the OpenAI API
/// and adopted as a standard across LLM providers.
#[derive(Debug, Serialize, bon::Builder)]
pub struct Error {
    /// The error details
    #[builder(field)]
    pub error: ErrorDetail,
    /// HTTP status code
    #[serde(skip)]
    pub status_code: StatusCode,
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        use headers::{HeaderMap, HeaderMapExt};

        if self.status_code.is_server_error() {
            tracing::error!(
                error.type = ?self.error.error_type,
                error.message = %self.error.message,
                error.code = ?self.error.code,
                error.param = ?self.error.param,
                "Internal server error",
            );
        }

        let body = serde_json::to_string(&self).unwrap();
        let mut headers = HeaderMap::new();
        headers.typed_insert(headers::ContentLength(body.len() as u64));
        headers.typed_insert(headers::ContentType::json());

        (self.status_code, headers, body).into_response()
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::from(CoreError::Json(e))
    }
}

impl From<CoreError> for Error {
    fn from(err: CoreError) -> Self {
        match err {
            CoreError::Json(e) => Error::builder()
                .status_code(StatusCode::BAD_REQUEST)
                .error_type(ErrorType::InvalidRequest)
                .message(e.to_string())
                .build(),
            CoreError::RequestBuild(msg) => Error::builder()
                .status_code(StatusCode::BAD_REQUEST)
                .error_type(ErrorType::InvalidRequest)
                .message(msg)
                .build(),
            CoreError::ProviderError { status, body } => Error::builder()
                .status_code(StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY))
                .error_type(ErrorType::Server)
                .message(body)
                .build(),
            CoreError::HttpClient(msg) => Error::builder()
                .status_code(StatusCode::BAD_GATEWAY)
                .error_type(ErrorType::Server)
                .message(msg)
                .build(),
            CoreError::Stream(msg) => Error::builder()
                .status_code(StatusCode::INTERNAL_SERVER_ERROR)
                .error_type(ErrorType::Server)
                .message(msg)
                .build(),
        }
    }
}

/// Error detail within an OpenAI-compatible error response.
#[derive(Debug, Default, Serialize)]
pub struct ErrorDetail {
    /// Human-readable error message
    pub message: String,
    /// Error type category (e.g., "invalid_request_error", "authentication_error")
    #[serde(rename = "type")]
    pub error_type: ErrorType,
    /// Optional error code for programmatic handling
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Optional parameter that caused the error
    #[serde(skip_serializing_if = "Option::is_none")]
    pub param: Option<String>,
}

/// Error type constants following OpenAI API conventions.
#[derive(Debug, Default, Serialize)]
pub enum ErrorType {
    #[serde(rename = "invalid_request_error")]
    InvalidRequest,
    #[serde(rename = "not_found_error")]
    NotFound,
    #[default]
    #[serde(rename = "server_error")]
    Server,
}

// Pre-built error responses for common cases

/// Create a `400 Bad Request` (or `413 Payload Too Large`, when the underlying
/// error is a `http_body_util::LengthLimitError`) response for a failed body read.
pub fn body_read_error(e: Box<dyn std::error::Error + Send + Sync>) -> Error {
    if e.downcast_ref::<http_body_util::LengthLimitError>()
        .is_some()
    {
        return Error::builder()
            .status_code(StatusCode::PAYLOAD_TOO_LARGE)
            .error_type(ErrorType::InvalidRequest)
            .message(format!(
                "request body exceeds {} bytes",
                crate::passthrough::MAX_BODY_BYTES
            ))
            .build();
    }
    Error::builder()
        .status_code(StatusCode::BAD_REQUEST)
        .error_type(ErrorType::InvalidRequest)
        .message(format!("failed to read request body: {e}"))
        .build()
}

/// Create a not found error response.
pub fn not_found(code: impl Into<String>, message: impl Into<String>) -> Error {
    Error::builder()
        .status_code(StatusCode::NOT_FOUND)
        .error_type(ErrorType::NotFound)
        .code(code)
        .message(message)
        .build()
}

const _: () = {
    use error_builder::*;

    impl<S: State> ErrorBuilder<S> {
        /// Set the error message.
        pub fn message(mut self, message: impl Into<String>) -> Self {
            self.error.message = message.into();
            self
        }

        /// Set the error type.
        pub fn error_type(mut self, error_type: ErrorType) -> Self {
            self.error.error_type = error_type;
            self
        }

        /// Set the error code.
        pub fn code(mut self, code: impl Into<String>) -> Self {
            self.error.code = Some(code.into());
            self
        }
    }
};
