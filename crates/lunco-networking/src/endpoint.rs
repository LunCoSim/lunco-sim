//! Validated WebTransport destinations shared by CLI, browser and command admission.

#[cfg(all(feature = "networking", not(target_family = "wasm")))]
use url::Host;
use url::Url;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetworkConfigError {
    Arguments(String),
    Endpoint(String),
    PageUrl(String),
}

impl std::fmt::Display for NetworkConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Arguments(message) => write!(formatter, "invalid network arguments: {message}"),
            Self::Endpoint(message) => write!(formatter, "invalid network endpoint: {message}"),
            Self::PageUrl(message) => write!(formatter, "invalid network page URL: {message}"),
        }
    }
}

impl std::error::Error for NetworkConfigError {}

/// An admitted host authority, HTTPS dial URL and exact nonzero transport port.
/// Construction validates with the same URL implementation used by wtransport.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectEndpoint {
    url: Url,
    address: String,
    port: u16,
}

impl ConnectEndpoint {
    /// Accept a hostname, IPv4 address or bracketed IPv6 address with an
    /// optional port. Only an omitted port uses the documented host-port default.
    pub fn parse(raw: &str) -> Result<Self, NetworkConfigError> {
        if raw.chars().any(char::is_control) {
            return Err(NetworkConfigError::Endpoint(
                "control characters are not allowed".into(),
            ));
        }
        let raw = raw.trim();
        if raw.is_empty()
            || raw.chars().any(char::is_whitespace)
            || raw.contains(['/', '\\', '?', '#', '@'])
            || raw.ends_with(':')
        {
            return Err(NetworkConfigError::Endpoint(
                "expected host[:port] or [IPv6][:port], without credentials, path, query or fragment".into(),
            ));
        }
        let mut url = Url::parse(&format!("https://{raw}"))
            .map_err(|error| NetworkConfigError::Endpoint(error.to_string()))?;
        let explicit_port = if raw.starts_with('[') {
            !raw.ends_with(']')
        } else {
            raw.contains(':')
        };
        let port = if explicit_port {
            url.port_or_known_default()
                .ok_or_else(|| NetworkConfigError::Endpoint("missing port".into()))?
        } else {
            lunco_core_session::DEFAULT_HOST_PORT
        };
        if port == 0 {
            return Err(NetworkConfigError::Endpoint(
                "port must be 1..=65535".into(),
            ));
        }
        url.set_port(Some(port))
            .map_err(|_| NetworkConfigError::Endpoint("endpoint cannot carry a port".into()))?;
        let host = url
            .host()
            .ok_or_else(|| NetworkConfigError::Endpoint("missing host".into()))?;
        let address = format!("{host}:{port}");
        Ok(Self { url, address, port })
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    #[cfg(all(feature = "networking", not(target_family = "wasm")))]
    pub(crate) fn is_bare_ip(&self) -> bool {
        matches!(self.url.host(), Some(Host::Ipv4(_) | Host::Ipv6(_)))
    }
}

impl std::fmt::Display for ConnectEndpoint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.address())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_endpoint_configuration_preserves_hostname_ipv4_ipv6_and_explicit_port() {
        for (raw, expected, port) in [
            ("host.example", "host.example:5888", 5888),
            ("HOST.example:443", "host.example:443", 443),
            ("127.0.0.1:1234", "127.0.0.1:1234", 1234),
            ("[::1]", "[::1]:5888", 5888),
            ("[2001:db8::1]:443", "[2001:db8::1]:443", 443),
            ("host.example:65535", "host.example:65535", 65535),
            ("münich.example", "xn--mnich-kva.example:5888", 5888),
        ] {
            let endpoint = ConnectEndpoint::parse(raw).unwrap();
            assert_eq!(endpoint.address(), expected);
            assert_eq!(endpoint.port(), port);
            assert_eq!(endpoint.url().port_or_known_default(), Some(port));
        }
    }

    #[test]
    fn network_endpoint_configuration_rejects_invalid_explicit_input() {
        for raw in [
            "",
            " ",
            ":5888",
            "host:",
            "host:0",
            "host:65536",
            "host:abc",
            "host:-1",
            "host:1:2",
            "::1",
            "[::1",
            "[::1]:",
            "[::1]:0",
            "user@host:1",
            "host/path",
            "host?query=1",
            "host#fragment",
            "host\\path",
            "https://host:1",
            "host name",
            "host\n:1",
            "host\t:1",
            "host\n",
        ] {
            assert!(ConnectEndpoint::parse(raw).is_err(), "{raw:?}");
        }
    }
}
