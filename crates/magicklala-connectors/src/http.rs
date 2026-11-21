use std::time::Duration;

use crate::ConnectorError;

pub(crate) fn http_client() -> reqwest::Client {
    reqwest::Client::builder().timeout(Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new())
}

pub(crate) fn setting_str(settings: &serde_json::Value, key: &str) -> Option<String> {
    settings.get(key).and_then(serde_json::Value::as_str).map(str::to_owned).filter(|value| !value.is_empty())
}

pub(crate) fn require_secret(secret: &Option<String>, what: &str) -> Result<String, ConnectorError> {
    secret.clone().filter(|value| !value.is_empty()).ok_or_else(|| ConnectorError::Config(format!("{what} is not configured")))
}

pub(crate) fn transport_failure(service: &str, error: reqwest::Error) -> ConnectorError {
    ConnectorError::Transport(format!("{service} request failed: {}", error.without_url()))
}

pub(crate) async fn reject_unsuccessful(service: &str, response: reqwest::Response) -> Result<serde_json::Value, ConnectorError> {
    let status = response.status();
    let retry_after = response.headers().get("retry-after").and_then(|value| value.to_str().ok()).map(str::to_owned);
    let body: serde_json::Value = response.json().await.unwrap_or(serde_json::Value::Null);
    if status.is_success() {
        return Ok(body);
    }
    if status.as_u16() == 429 {
        let wait = retry_after.or_else(|| body.get("retry_after").map(ToString::to_string)).unwrap_or_else(|| "unknown".to_owned());
        return Err(ConnectorError::Transport(format!("{service} rate limited the request; retry after {wait}")));
    }
    Err(ConnectorError::Transport(format!("{service} responded with HTTP {}", status.as_u16())))
}
