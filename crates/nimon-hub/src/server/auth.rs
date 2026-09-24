//! Token authentication for API writes and edge WebSockets.

use axum::{
    extract::{Request, State},
    http::{header, HeaderMap, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Json, Response},
};

use crate::server::HubState;

/// Constant-time byte comparison (length is not secret).
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The token of an `Authorization: Bearer <token>` header.
pub fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?.trim();
    let (scheme, token) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| token.trim())
        .filter(|t| !t.is_empty())
}

/// True when `provided` matches `expected` (constant time).
pub fn token_matches(provided: Option<&str>, expected: &str) -> bool {
    provided
        .map(|p| constant_time_eq(p.as_bytes(), expected.as_bytes()))
        .unwrap_or(false)
}

pub(crate) fn unauthorized(message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Bearer")],
        Json(serde_json::json!({ "error": message })),
    )
        .into_response()
}

/// Middleware: when `auth.api_token` is set, every non-GET/HEAD/OPTIONS
/// request under `/api/` needs `Authorization: Bearer <token>`.
pub async fn api_auth(State(state): State<HubState>, request: Request, next: Next) -> Response {
    if let Some(expected) = state.auth().api_token.as_deref() {
        let read_only = matches!(
            *request.method(),
            Method::GET | Method::HEAD | Method::OPTIONS
        );
        if request.uri().path().starts_with("/api/")
            && !read_only
            && !token_matches(bearer_token(request.headers()), expected)
        {
            return unauthorized("missing or invalid API token");
        }
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constant_time_eq() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secreT"));
        assert!(!constant_time_eq(b"secret", b"secret2"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn test_bearer_token() {
        let mut headers = HeaderMap::new();
        assert_eq!(bearer_token(&headers), None);
        headers.insert(header::AUTHORIZATION, "Bearer abc".parse().unwrap());
        assert_eq!(bearer_token(&headers), Some("abc"));
        headers.insert(header::AUTHORIZATION, "bearer  xyz ".parse().unwrap());
        assert_eq!(bearer_token(&headers), Some("xyz"));
        headers.insert(header::AUTHORIZATION, "Basic abc".parse().unwrap());
        assert_eq!(bearer_token(&headers), None);
        assert!(token_matches(Some("t"), "t"));
        assert!(!token_matches(None, "t"));
    }
}
