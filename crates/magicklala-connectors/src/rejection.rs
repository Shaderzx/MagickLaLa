use crate::ConnectorError;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InboundRejection {
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    #[error("invalid inbound request: {0}")]
    Invalid(String),
    #[error("connector is not running")]
    NotRunning,
}

impl From<InboundRejection> for ConnectorError {
    fn from(rejection: InboundRejection) -> Self {
        match rejection {
            InboundRejection::NotRunning => ConnectorError::NotRunning,
            other => ConnectorError::Config(other.to_string()),
        }
    }
}
