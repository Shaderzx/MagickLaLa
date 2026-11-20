use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::extract::{Query, Request, State};
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE, HOST, ORIGIN, WWW_AUTHENTICATE};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::AppState;
use crate::error::ApiError;

pub const CORS_ORIGINS_ENV: &str = "MAGICKLALA_CORS_ORIGINS";
pub const API_TOKEN_ENV: &str = "MAGICKLALA_API_TOKEN";
pub const ALLOWED_HOSTS_ENV: &str = "MAGICKLALA_ALLOWED_HOSTS";
pub const LOCAL_STUDIO_ORIGINS: [&str; 4] = ["http://127.0.0.1:3000", "http://localhost:3000", "http://127.0.0.1:4242", "http://localhost:4242"];
pub const LOOPBACK_HOSTS: [&str; 4] = ["localhost", "127.0.0.1", "[::1]", "::1"];
pub const PUBLIC_PATHS: [&str; 2] = ["/health", "/api/v1/webhooks/{path}"];
pub const WS_TICKET_TTL: Duration = Duration::from_secs(30);
pub const MAX_LIVE_WS_TICKETS: usize = 1_024;
const CORS_MAX_AGE_SECONDS: u64 = 600;
const WEBSOCKET_PATH: &str = "/ws";

pub fn axum_route(openapi_path: &str) -> String {
    openapi_path.replace("{path}", "{*path}")
}

#[derive(Clone, PartialEq, Eq)]
pub struct AccessPolicy {
    pub cors_origins: Vec<String>,
    pub api_token: Option<String>,
    pub allowed_hosts: Vec<String>,
}

impl std::fmt::Debug for AccessPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccessPolicy")
            .field("cors_origins", &self.cors_origins)
            .field("api_token", &self.api_token.as_ref().map(|_| "<redacted>"))
            .field("allowed_hosts", &self.allowed_hosts)
            .finish()
    }
}

impl Default for AccessPolicy {
    fn default() -> Self {
        Self { cors_origins: LOCAL_STUDIO_ORIGINS.map(str::to_owned).to_vec(), api_token: None, allowed_hosts: Vec::new() }
    }
}

impl AccessPolicy {
    pub fn from_env() -> Self {
        let cors_origins = std::env::var(CORS_ORIGINS_ENV).ok().map_or_else(|| Self::default().cors_origins, |raw| parse_origins(&raw));
        let api_token = std::env::var(API_TOKEN_ENV).ok().map(|token| token.trim().to_owned()).filter(|token| !token.is_empty());
        let allowed_hosts = std::env::var(ALLOWED_HOSTS_ENV).ok().map(|raw| parse_hosts(&raw)).unwrap_or_default();
        Self { cors_origins, api_token, allowed_hosts }
    }

    pub fn with_api_token(mut self, token: impl Into<String>) -> Self {
        self.api_token = Some(token.into()).filter(|token: &String| !token.trim().is_empty());
        self
    }

    pub fn with_cors_origins(mut self, origins: &str) -> Self {
        self.cors_origins = parse_origins(origins);
        self
    }

    pub fn with_allowed_hosts(mut self, hosts: &str) -> Self {
        self.allowed_hosts.extend(parse_hosts(hosts));
        self
    }

    pub fn with_bind(mut self, bind: SocketAddr) -> Self {
        if !bind.ip().is_unspecified() && !bind.ip().is_loopback() {
            let host = match bind {
                SocketAddr::V4(v4) => v4.ip().to_string(),
                SocketAddr::V6(v6) => format!("[{}]", v6.ip()),
            };
            self.allowed_hosts.push(host);
        }
        self
    }

    pub fn requires_token(&self) -> bool {
        self.api_token.is_some()
    }

    pub fn host_allowed(&self, raw: &str) -> bool {
        let raw = raw.trim().to_ascii_lowercase();
        let host = host_without_port(&raw);
        LOOPBACK_HOSTS.contains(&host) || self.allowed_hosts.iter().any(|allowed| *allowed == raw || allowed == host)
    }

    pub fn origin_allowed(&self, origin: &str, host: Option<&str>) -> bool {
        let origin = origin.trim().trim_end_matches('/');
        let same_origin = host.is_some_and(|host| ["http://", "https://"].iter().any(|scheme| origin.eq_ignore_ascii_case(&format!("{scheme}{host}"))));
        same_origin || self.cors_origins.iter().any(|allowed| allowed.eq_ignore_ascii_case(origin))
    }
}

fn host_without_port(raw: &str) -> &str {
    if raw.starts_with('[') {
        return raw.find(']').map_or(raw, |end| &raw[..=end]);
    }
    match raw.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') && port.chars().all(|c| c.is_ascii_digit()) => host,
        _ => raw,
    }
}

pub fn parse_hosts(raw: &str) -> Vec<String> {
    raw.split(',').map(|host| host.trim().to_ascii_lowercase()).filter(|host| !host.is_empty() && *host != "*").collect()
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
        .allow_headers([CONTENT_TYPE, AUTHORIZATION, HeaderName::from_static("x-magicklala-signature"), HeaderName::from_static("x-magicklala-timestamp")])
        .max_age(std::time::Duration::from_secs(CORS_MAX_AGE_SECONDS))
}

pub fn request_host(headers: &HeaderMap, uri: &Uri) -> Option<String> {
    headers.get(HOST).and_then(|value| value.to_str().ok()).map(str::to_owned).or_else(|| uri.authority().map(|authority| authority.as_str().to_owned()))
}

pub fn origin_rejected(policy: &AccessPolicy, headers: &HeaderMap, uri: &Uri) -> bool {
    let Some(origin) = headers.get(ORIGIN) else { return false };
    let host = request_host(headers, uri);
    origin.to_str().map_or(true, |origin| !policy.origin_allowed(origin, host.as_deref()))
}

pub async fn require_known_host(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let host = request_host(request.headers(), request.uri());
    if host.as_deref().is_some_and(|host| state.access.host_allowed(host)) {
        return next.run(request).await;
    }
    let shown = host.unwrap_or_else(|| "<missing>".to_owned());
    ApiError::new(StatusCode::MISDIRECTED_REQUEST, "unknown_host", format!("host {shown:?} is not served here; add it to {ALLOWED_HOSTS_ENV} to allow it"))
        .into_response()
}

#[derive(Debug, Default)]
pub struct WsTickets {
    live: Mutex<HashMap<[u8; 32], Instant>>,
}

impl WsTickets {
    pub fn new() -> Self {
        Self::default()
    }

    fn digest(ticket: &str) -> [u8; 32] {
        Sha256::digest(ticket.trim().as_bytes()).into()
    }

    pub fn issue(&self) -> String {
        let ticket = hex::encode(rand::random::<[u8; 32]>());
        let now = Instant::now();
        let mut live = self.live.lock().unwrap_or_else(PoisonError::into_inner);
        live.retain(|_, expires| *expires > now);
        if live.len() >= MAX_LIVE_WS_TICKETS
            && let Some(oldest) = live.iter().min_by_key(|(_, expires)| **expires).map(|(key, _)| *key)
        {
            live.remove(&oldest);
        }
        live.insert(Self::digest(&ticket), now + WS_TICKET_TTL);
        ticket
    }

    pub fn redeem(&self, ticket: &str) -> bool {
        let mut live = self.live.lock().unwrap_or_else(PoisonError::into_inner);
        live.remove(&Self::digest(ticket)).is_some_and(|expires| expires > Instant::now())
    }
}

#[derive(Deserialize)]
struct TicketQuery {
    ticket: Option<String>,
}

fn bearer(request: &Request) -> Option<String> {
    let header = request.headers().get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = header.trim().split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim().to_owned())
}

fn query_ticket(uri: &Uri) -> Option<String> {
    Query::<TicketQuery>::try_from_uri(uri).ok()?.0.ticket.filter(|ticket| !ticket.is_empty())
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

fn authorized(state: &AppState, expected: &str, request: &Request) -> bool {
    if let Some(token) = bearer(request) {
        return tokens_match(expected, &token);
    }
    request.uri().path() == WEBSOCKET_PATH && query_ticket(request.uri()).is_some_and(|ticket| state.tickets.redeem(&ticket))
}

pub async fn require_token(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let Some(expected) = state.access.api_token.as_deref() else { return next.run(request).await };
    if authorized(&state, expected, &request) { next.run(request).await } else { unauthorized() }
}
