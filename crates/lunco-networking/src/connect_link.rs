//! Connect deep-links — the single source of truth for the two URL forms that
//! carry a `JoinServer` (address + optional self-signed cert digest):
//!
//! - **Native scheme** `luncosim://connect?address=HOST:PORT&digest=HEX` — what
//!   the OS hands a registered handler when a `luncosim://` link is clicked
//!   (Telegram, a browser, a file manager). Parsed by [`parse_native`] at
//!   startup / over the single-instance IPC, routed to a *confirmed* connect.
//! - **Web URL** `https://lunica.lunco.space/?connect=HOST:PORT#HEX` — a plain
//!   https link that the wasm build already auto-connects from
//!   ([`crate::NetworkMode::from_url`] reads `?connect=`, the digest rides the
//!   `#hash` via `client_cert_digest`). Built here so the host's *Copy invite
//!   link* button and the web path agree on the format.
//!
//! Both forms decode to the same [`ConnectLink`], which the UI turns into the
//! typed [`JoinServer`](crate::client::JoinServer) command — after a user
//! confirm, since an unsolicited link must not silently redirect a session.
//!
//! Pure module (no bevy, no I/O) so it unit-tests in isolation and is reused by
//! the clipboard command, the native arg parser, and (later) the IPC forwarder.

/// Custom URL scheme registered with the OS for native deep-links.
pub const SCHEME: &str = "luncosim";

/// Default public web origin for an invite link built off-host (no live
/// `window.location`). Mirrors `model_share::PUBLIC_BASE`.
pub const PUBLIC_BASE: &str = "https://lunica.lunco.space/";

/// A decoded connect request: where to dial and (optionally) which self-signed
/// cert digest to pin. Mirrors the fields of [`crate::client::JoinServer`].
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ConnectLink {
    /// `host:port` — a hostname or `ip:port`.
    pub address: String,
    /// Self-signed cert SHA-256 digest (bare/colon hex), or empty for CA.
    pub digest: String,
}

/// Build the **native scheme** link `luncosim://connect?address=…&digest=…`.
/// The digest is omitted when empty (CA-cert / native bare-IP host).
pub fn native_url(address: &str, digest: &str) -> String {
    let mut url = format!("{SCHEME}://connect?address={}", encode_component(address));
    if !digest.is_empty() {
        url.push_str("&digest=");
        url.push_str(&encode_component(digest));
    }
    url
}

/// Build the **web** invite link `<base>?connect=ADDR#DIGEST`. `base` is the
/// live page origin on wasm, else [`PUBLIC_BASE`]. The digest rides the URL
/// fragment (never sent to the server) so the browser pins a self-signed host.
pub fn web_url(base: &str, address: &str, digest: &str) -> String {
    let base = if base.is_empty() { PUBLIC_BASE } else { base };
    // Strip any existing query/fragment so re-building off the live location is
    // idempotent.
    let base = base.split(['?', '#']).next().unwrap_or(base);
    let mut url = format!("{base}?connect={}", encode_component(address));
    if !digest.is_empty() {
        url.push('#');
        url.push_str(&encode_component(digest));
    }
    url
}

/// Parse a native `luncosim://connect?address=…&digest=…` link. Returns `None`
/// for a non-`luncosim` scheme, the wrong action, a missing address, or a
/// digest that isn't bare/colon hex (see [`is_hex_digest`]). Tolerant
/// of an absent digest and of `luncosim:connect?…` (no `//`, as some launchers
/// hand it over).
pub fn parse_native(url: &str) -> Option<ConnectLink> {
    let rest = url
        .strip_prefix(&format!("{SCHEME}://"))
        .or_else(|| url.strip_prefix(&format!("{SCHEME}:")))?;
    // `connect?address=…&digest=…` (a trailing `/` after the action is fine).
    let (action, query) = match rest.split_once('?') {
        Some((a, q)) => (a, q),
        None => (rest, ""),
    };
    if action.trim_end_matches('/') != "connect" {
        return None;
    }
    let mut link = ConnectLink::default();
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        match k {
            "address" => link.address = decode_component(v),
            "digest" => link.digest = decode_component(v),
            _ => {}
        }
    }
    if link.address.trim().is_empty() {
        return None;
    }
    // The digest is only ever a SHA-256 cert fingerprint (bare or colon-separated
    // hex) or absent — reject anything else so a hostile link can't smuggle an
    // arbitrary string into the cert-pinning path.
    if !is_hex_digest(&link.digest) {
        return None;
    }
    Some(link)
}

/// True for the digest forms a connect link may carry: empty (CA cert) or
/// bare/colon-separated hex (a SHA-256 cert fingerprint).
fn is_hex_digest(s: &str) -> bool {
    !s.contains("::") && s.chars().all(|c| c.is_ascii_hexdigit() || c == ':')
}

/// Minimal percent-encoder for the handful of characters that actually need it
/// in our values (`host:port` and hex digests are otherwise URL-safe). Avoids a
/// urlencoding dependency for two tiny fields. `:` is left as-is — it's legal in
/// a query component and keeps `host:port` readable.
fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b':' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Inverse of [`encode_component`]: decode `%XX` escapes, leave everything else.
fn decode_component(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_round_trip_with_digest() {
        let url = native_url("192.168.10.91:5888", "ab12cd");
        assert_eq!(
            url,
            "luncosim://connect?address=192.168.10.91:5888&digest=ab12cd"
        );
        let link = parse_native(&url).unwrap();
        assert_eq!(link.address, "192.168.10.91:5888");
        assert_eq!(link.digest, "ab12cd");
    }

    #[test]
    fn native_no_digest() {
        let url = native_url("host.example:5888", "");
        assert_eq!(url, "luncosim://connect?address=host.example:5888");
        let link = parse_native(&url).unwrap();
        assert_eq!(link.address, "host.example:5888");
        assert!(link.digest.is_empty());
    }

    #[test]
    fn parse_tolerates_no_slashes_and_trailing_slash() {
        let link = parse_native("luncosim:connect/?address=10.0.0.5:5888").unwrap();
        assert_eq!(link.address, "10.0.0.5:5888");
    }

    #[test]
    fn parse_rejects_non_hex_digest() {
        assert!(parse_native("luncosim://connect?address=h:1&digest=nothexz").is_none());
        assert!(parse_native("luncosim://connect?address=h:1&digest=ab%20cd").is_none());
        // Colon-separated fingerprint form stays accepted.
        let link = parse_native("luncosim://connect?address=h:1&digest=AB:CD:12").unwrap();
        assert_eq!(link.digest, "AB:CD:12");
    }

    #[test]
    fn parse_rejects_foreign_scheme_action_or_empty_address() {
        assert!(parse_native("https://lunica.lunco.space/?connect=x").is_none());
        assert!(parse_native("luncosim://host?address=x").is_none()); // wrong action
        assert!(parse_native("luncosim://connect?digest=ab").is_none()); // no address
    }

    #[test]
    fn web_url_idempotent_and_fragments_digest() {
        let u = web_url("https://lunica.lunco.space/", "192.168.10.91:5888", "ab12");
        assert_eq!(
            u,
            "https://lunica.lunco.space/?connect=192.168.10.91:5888#ab12"
        );
        // Re-building off a live location that already carries query/fragment.
        let u2 = web_url(
            "https://lunica.lunco.space/?connect=old#deadbeef",
            "h:1",
            "",
        );
        assert_eq!(u2, "https://lunica.lunco.space/?connect=h:1");
    }

    #[test]
    fn empty_base_falls_back_to_public() {
        let u = web_url("", "h:5888", "");
        assert!(u.starts_with(PUBLIC_BASE));
    }
}
