//! Native client for the LunCoSim command API.
//!
//! The client owns endpoint construction, HTTP request/response handling, and
//! transport errors. It does not know about Rhai, Bevy, or any particular
//! command. Frontends such as `lunco-rhai-repl` provide a command name and
//! parameters through the shared [`lunco_api_contracts`] envelope.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use lunco_api_contracts::{ApiRequestEnvelope, ApiResponseEnvelope, COMMANDS_PATH};
use std::fmt;
use std::time::Duration;

/// An API endpoint configured by the caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiEndpoint {
    base_url: String,
}

impl ApiEndpoint {
    /// Create an endpoint from a caller-provided HTTP API base URL.
    ///
    /// URL syntax is validated by the HTTP client at request time. Keeping the
    /// endpoint as configuration rather than formatting HTTP by hand lets the
    /// same client work with a loopback port, a forwarded endpoint, or a
    /// deployment URL.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
        }
    }

    /// Create the documented local development endpoint.
    pub fn loopback(port: u16) -> Self {
        Self::new(format!("http://{}:{port}", std::net::Ipv4Addr::LOCALHOST))
    }

    /// Return the configured base URL.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn commands_url(&self) -> String {
        format!("{}{COMMANDS_PATH}", self.base_url)
    }
}

impl fmt::Display for ApiEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.base_url)
    }
}

/// Errors produced while sending or decoding an API request.
#[derive(Debug)]
pub enum ApiClientError {
    /// The HTTP library could not complete the request.
    Transport(ureq::Error),
    /// The server returned bytes that are not a response envelope.
    InvalidResponse(serde_json::Error),
    /// The request contract could not be serialized.
    InvalidRequest(serde_json::Error),
    /// The endpoint was configured without a usable base URL.
    EmptyEndpoint,
}

impl fmt::Display for ApiClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => write!(formatter, "transport error: {error}"),
            Self::InvalidResponse(error) => write!(formatter, "invalid API response: {error}"),
            Self::InvalidRequest(error) => write!(formatter, "invalid API request: {error}"),
            Self::EmptyEndpoint => formatter.write_str("API endpoint is empty"),
        }
    }
}

impl std::error::Error for ApiClientError {}

/// Generic synchronous client for the command API.
#[derive(Clone)]
pub struct ApiClient {
    endpoint: ApiEndpoint,
    agent: ureq::Agent,
}

impl fmt::Debug for ApiClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ApiClient")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl ApiClient {
    /// Construct a client with the standard request timeout.
    pub fn new(endpoint: ApiEndpoint) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(120)))
            .http_status_as_error(false)
            .build()
            .into();
        Self { endpoint, agent }
    }

    /// Return the configured endpoint.
    pub fn endpoint(&self) -> &ApiEndpoint {
        &self.endpoint
    }

    /// Send one generic API request and decode its canonical response envelope.
    pub fn execute(
        &self,
        request: &ApiRequestEnvelope,
    ) -> Result<ApiResponseEnvelope, ApiClientError> {
        if self.endpoint.base_url().is_empty() {
            return Err(ApiClientError::EmptyEndpoint);
        }
        let body = serde_json::to_vec(request).map_err(ApiClientError::InvalidRequest)?;
        let mut response = self
            .agent
            .post(&self.endpoint.commands_url())
            .header("Content-Type", "application/json")
            .send(body)
            .map_err(ApiClientError::Transport)?;
        let body = response
            .body_mut()
            .read_to_vec()
            .map_err(ApiClientError::Transport)?;
        serde_json::from_slice(&body).map_err(ApiClientError::InvalidResponse)
    }
}
