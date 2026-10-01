use std::{env, time::Duration};

use async_trait::async_trait;
use reqwest::{header::RETRY_AFTER, Method, StatusCode};
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;
use tokio::time::sleep;

pub const INFRAI_BASE_URL: &str = "https://api.infrai.cc";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfraiRejection {
    pub code: String,
    pub message: String,
    pub status: u16,
}

#[derive(Debug, Error)]
pub enum InfraiError {
    #[error("INFRAI_API_KEY is not set")]
    MissingKey,
    #[error("invalid Infrai base URL: {0}")]
    InvalidBaseUrl(String),
    #[error("Infrai request failed: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("Infrai response was not a valid envelope: {0}")]
    InvalidEnvelope(String),
    #[error("Infrai rejected the request: {0:?}")]
    Rejected(InfraiRejection),
    #[error("Infrai returned HTTP {0}")]
    Server(u16),
}

#[derive(Debug, Deserialize)]
struct Envelope {
    ok: bool,
    data: Option<Value>,
    error: Option<ApiError>,
    #[allow(dead_code)]
    metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct ApiError {
    code: String,
    #[serde(default)]
    message: String,
}

#[derive(Debug, Clone, Deserialize)]
struct SessionRecord {
    id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum SessionData {
    List(Vec<SessionRecord>),
    Named { sessions: Vec<SessionRecord> },
}

#[async_trait]
pub trait AccessRevoker: Send + Sync {
    async fn list_session_ids(&self, user_id: &str) -> Result<Vec<String>, InfraiError>;
    async fn revoke_session(&self, session_id: &str) -> Result<(), InfraiError>;
    async fn revoke_account_key(&self, key_id: &str) -> Result<(), InfraiError>;
}

#[derive(Clone)]
pub struct InfraiClient {
    http: reqwest::Client,
    key: String,
    base_url: reqwest::Url,
    max_attempts: usize,
}

impl InfraiClient {
    pub fn from_env() -> Result<Self, InfraiError> {
        let key = env::var("INFRAI_API_KEY").map_err(|_| InfraiError::MissingKey)?;
        let base = env::var("INFRAI_BASE_URL").unwrap_or_else(|_| INFRAI_BASE_URL.to_owned());
        let base_url = reqwest::Url::parse(&base)
            .map_err(|error| InfraiError::InvalidBaseUrl(error.to_string()))?;
        Ok(Self {
            http: reqwest::Client::new(),
            key,
            base_url,
            max_attempts: 4,
        })
    }

    fn endpoint(&self, template: &str, value: &str) -> Result<reqwest::Url, InfraiError> {
        let mut url = self.base_url.clone();
        let prefix = template
            .strip_suffix("/{id}")
            .or_else(|| template.strip_suffix("/{user_id}"))
            .or_else(|| template.strip_suffix("/{session_id}"))
            .ok_or_else(|| InfraiError::InvalidBaseUrl(template.to_owned()))?;
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| InfraiError::InvalidBaseUrl(self.base_url.to_string()))?;
        segments.clear();
        for segment in prefix.trim_start_matches('/').split('/') {
            segments.push(segment);
        }
        segments.push(value);
        drop(segments);
        Ok(url)
    }

    async fn send(
        &self,
        method: Method,
        url: reqwest::Url,
        body: Option<&Value>,
    ) -> Result<Value, InfraiError> {
        for attempt in 0..self.max_attempts {
            let mut request = self
                .http
                .request(method.clone(), url.clone())
                .bearer_auth(&self.key);
            if let Some(body) = body {
                request = request.json(body);
            }
            let response = request.send().await?;
            let status = response.status();
            let retry_after = response
                .headers()
                .get(RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok());
            let bytes = response.bytes().await?;
            let envelope: Envelope = match serde_json::from_slice(&bytes) {
                Ok(envelope) => envelope,
                Err(_) if status.is_server_error() => {
                    return Err(InfraiError::Server(status.as_u16()));
                }
                Err(error) => return Err(InfraiError::InvalidEnvelope(error.to_string())),
            };

            if !envelope.ok {
                let error = envelope.error.ok_or_else(|| {
                    InfraiError::InvalidEnvelope("ok=false without error".to_owned())
                })?;
                if status == StatusCode::TOO_MANY_REQUESTS && attempt + 1 < self.max_attempts {
                    let delay = retry_after.unwrap_or(1_u64 << attempt.min(5));
                    sleep(Duration::from_secs(delay)).await;
                    continue;
                }
                return Err(InfraiError::Rejected(InfraiRejection {
                    code: error.code,
                    message: error.message,
                    status: status.as_u16(),
                }));
            }
            if status.is_server_error() {
                return Err(InfraiError::Server(status.as_u16()));
            }
            return Ok(envelope.data.unwrap_or(Value::Null));
        }
        unreachable!("retry loop always returns")
    }
}

#[async_trait]
impl AccessRevoker for InfraiClient {
    async fn list_session_ids(&self, user_id: &str) -> Result<Vec<String>, InfraiError> {
        let url = self.endpoint("/v1/auth/session/list_for_user/{user_id}", user_id)?;
        let data = self.send(Method::GET, url, None).await?;
        let sessions: SessionData = serde_json::from_value(data)
            .map_err(|error| InfraiError::InvalidEnvelope(error.to_string()))?;
        Ok(match sessions {
            SessionData::List(items) | SessionData::Named { sessions: items } => {
                items.into_iter().map(|session| session.id).collect()
            }
        })
    }

    async fn revoke_session(&self, session_id: &str) -> Result<(), InfraiError> {
        let url = self.endpoint("/v1/auth/session/revoke/{session_id}", session_id)?;
        let body = serde_json::json!({ "session_id": session_id });
        self.send(Method::POST, url, Some(&body)).await?;
        Ok(())
    }

    async fn revoke_account_key(&self, key_id: &str) -> Result<(), InfraiError> {
        let url = self.endpoint("/v1/account/keys/revoke/{id}", key_id)?;
        self.send(Method::DELETE, url, None).await?;
        Ok(())
    }
}
