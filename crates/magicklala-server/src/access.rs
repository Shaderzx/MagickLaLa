use axum::extract::{Query, Request, State};
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE, WWW_AUTHENTICATE};
use axum::http::{HeaderName, HeaderValue, Method, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::AppState;
use crate::error::ApiError;

pub const CORS_ORIGINS_ENV: &str = "MAGICKLALA_CORS_ORIGINS";
pub const API_TOKEN_ENV: &str = "MAGICKLALA_API_TOKEN";
pub const LOCAL_STUDIO_ORIGINS: [&str; 4] = ["http://127.0.0.1:3000", "http://localhost:3000", "http://127.0.0.1:4242", "http://localhost:4242"];
const CORS_MAX_AGE_SECONDS: u64 = 600;

#[derive(Clone, PartialEq, Eq)]
pub struct AccessPolicy {
    pub cors_origins: Vec<String>,
    pub api_token: Option<String>,
}

impl std::fmt::Debug for AccessPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccessPolicy").field("cors_origins", &self.cors_origins).field("api_token", &self.api_token.as_ref().map(|_| "<redacted>")).finish()
    }
}

impl Default for AccessPolicy {
    fn default() -> Self {
        Self { cors_origins: LOCAL_STUDIO_ORIGINS.map(str::to_owned).to_vec(), api_token: None }
    }
}

impl AccessPolicy {
    pub fn from_env() -> Self {
        let cors_origins = std::env::var(CORS_ORIGINS_ENV).ok().map_or_else(|| Self::default().cors_origins, |raw| parse_origins(&raw));
        let api_token = std::env::var(API_TOKEN_ENV).ok().map(|token| token.trim().to_owned()).filter(|token| !token.is_empty());
        Self { cors_origins, api_token }
    }

    pub fn with_api_token(mut self, token: impl Into<String>) -> Self {
        self.api_token = Some(token.into()).filter(|token: &String| !token.trim().is_empty());
        self
    }

    pub fn with_cors_origins(mut self, origins: &str) -> Self {
        self.cors_origins = parse_origins(origins);
        self
    }

    pub fn requires_token(&self) -> bool {
        self.api_token.is_some()
    }
}

pub fn parse_origins(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|origin| origin.trim().trim_end_matches('/'))
        .filter(|origin| !origin.is_empty() && *origin != "*" && !origin.eq_ignore_ascii_case("null"))
        .filter(|origin| origin.starts_with("http://") || origin.starts_with("https://"))
        .map(str::to_owned)
        .collect()
}

pub fn cors_layer(policy: &AccessPolicy) -> CorsLayer {
    let origins: Vec<HeaderValue> = policy.cors_origins.iter().filter_map(|origin| HeaderValue::from_str(origin).ok()).collect();
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::PATCH, Method::DELETE, Method::OPTIONS])
        .allow_headers([CONTENT_TYPE, AUTHORIZATION, HeaderName::from_static("x-magicklala-signature")])
        .max_age(std::time::Duration::from_secs(CORS_MAX_AGE_SECONDS))
}

#[derive(Deserialize)]
struct TokenQuery {
    token: Option<String>,
}

fn bearer(request: &Request) -> Option<String> {
    let header = request.headers().get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = header.trim().split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim().to_owned())
}

fn query_token(uri: &Uri) -> Option<String> {
    Query::<TokenQuery>::try_from_uri(uri).ok()?.0.token
}

fn presented_token(request: &Request) -> Option<String> {
    bearer(request).or_else(|| if request.uri().path() == "/ws" { query_token(request.uri()) } else { None })
}

fn digest(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

pub fn tokens_match(expected: &str, presented: &str) -> bool {
    digest(expected).iter().zip(digest(presented)).fold(0_u8, |difference, (left, right)| difference | (left ^ right)) == 0
}

pub fn unauthorized() -> Response {
    let error = ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized", "a valid bearer token is required");
    ([(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"))], error).into_response()
}

pub async fn require_token(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let Some(expected) = state.access.api_token.as_deref() else { return next.run(request).await };
    match presented_token(&request) {
        Some(token) if tokens_match(expected, &token) => next.run(request).await,
        _ => unauthorized(),
    }
}
