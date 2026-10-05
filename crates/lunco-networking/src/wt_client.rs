//! URL-dialing WebTransport client IO — native **and** browser.
//!
//! lightyear's built-in `WebTransportClientIo` builds its dial URL as
//! `https://{SocketAddr}` (IP-only) from a `PeerAddr`. A CA cert issued for a
//! hostname (e.g. `sandbox.lunco.space`) will never validate against a bare IP
//! because the SNI/SAN won't match. This module replaces lightyear's observer
//! with one that dials a URL **we** control (`https://sandbox.lunco.space:5888`)
//! on **both** native and browser, letting the OS/browser resolve DNS and
//! validate the domain cert normally. A self-signed dev cert still pins via
//! `certificate_digest` (empty ⇒ normal CA validation).
//!
//! We deliberately do **not** add `AeronetPlugin` / aeronet's
//! `WebTransportClientPlugin` here — lightyear's `ClientPlugins` already adds
//! them (the `webtransport` feature). We only register our own `link` observer,
//! which fires for entities carrying [`WtUrlClientIo`] (lightyear's fires for
//! `WebTransportClientIo`; the two coexist without conflict).

use aeronet_webtransport::client::{ClientConfig, WebTransportClient};
use bevy::prelude::*;
use lightyear::prelude::{LinkStart, Linked, Linking};
use lightyear_aeronet::AeronetLinkOf;

/// Component on the client entity: the full WebTransport URL to dial plus the
/// optional self-signed cert digest. An absent digest means:
/// - browser: no `serverCertificateHashes` → normal CA validation.
/// - native: uses the system CA store → normal CA validation.
///
/// A validated 32-byte digest pins a specific self-signed cert.
#[derive(Component)]
pub(crate) struct WtUrlClientIo {
    /// Full URL, e.g. `https://sandbox.lunco.space:5888`.
    pub url: String,
    /// Validated SHA-256 pin. `None` selects the documented unpinned mode.
    pub certificate_digest: Option<[u8; 32]>,
}

/// Registers the URL-dialing link observer on both native and wasm. Add once;
/// lightyear's WebTransport plugin (already pulled in by `ClientPlugins`) owns
/// the aeronet session setup.
pub(crate) struct WtUrlClientPlugin;

impl Plugin for WtUrlClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(link);
    }
}

/// On `LinkStart` for a [`WtUrlClientIo`] entity, spawn the aeronet WebTransport
/// session against our URL. Mirrors `lightyear_webtransport`'s `link`, minus the
/// `PeerAddr` requirement (we carry the full URL instead).
fn link(
    trigger: On<LinkStart>,
    query: Query<(Entity, &WtUrlClientIo), (Without<Linking>, Without<Linked>)>,
    mut commands: Commands,
) {
    if let Ok((entity, io)) = query.get(trigger.entity) {
        let url = io.url.clone();
        let digest = io.certificate_digest;
        commands.queue(move |world: &mut World| {
            if world.get_entity(entity).is_err() {
                return;
            }
            let config = client_config(&url, digest);
            let entity_mut = world.spawn((AeronetLinkOf(entity), Name::from("WtUrlClient")));
            // Native: `into_options()` converts the URL string to wtransport's
            // `ConnectOptions`, which preserves the hostname for SNI and DNS
            // resolution — unlike `PeerAddr` which forces an IP-literal URL.
            // Browser: the URL string is passed directly (xwt_web handles it).
            #[cfg(not(target_family = "wasm"))]
            {
                use aeronet_webtransport::wtransport::endpoint::IntoConnectOptions;
                WebTransportClient::connect(config, url.into_options()).apply(entity_mut);
            }
            #[cfg(target_family = "wasm")]
            {
                WebTransportClient::connect(config, url).apply(entity_mut);
            }
        });
    }
}

/// Build the `ClientConfig` for the given URL.
///
/// - Absent digest: use system CA store (native hostname) / no `serverCertificateHashes`
///   (browser) → normal CA chain validation. **Production path.**
/// - Present digest: pin a specific self-signed cert SHA-256.
///   **Dev/localhost only.**
fn client_config(url: &str, cert_hash: Option<[u8; 32]>) -> ClientConfig {
    #[cfg(not(target_family = "wasm"))]
    {
        native_client_config(url, cert_hash)
    }
    #[cfg(target_family = "wasm")]
    {
        let _ = url;
        wasm_client_config(cert_hash)
    }
}

/// Whether the `https://host:port` URL's host is a **bare IP literal** (v4 or
/// v6) rather than a DNS name. A bare IP triggers the no-cert-validation direct
/// path (a self-signed server over LAN/dev needs no CA cert and no digest);
/// hostnames never do. IPv6 literals are bracketed (`https://[::1]:5888`).
#[cfg(not(target_family = "wasm"))]
fn url_host_is_bare_ip(url: &str) -> bool {
    let after_scheme = url.strip_prefix("https://").unwrap_or(url);
    let host = if let Some(rest) = after_scheme.strip_prefix('[') {
        rest.split(']').next().unwrap_or("") // [::1]:5888 → ::1
    } else {
        after_scheme.split(':').next().unwrap_or("") // 192.168.0.5:5888 → 192.168.0.5
    };
    host.parse::<std::net::IpAddr>().is_ok()
}

/// Native client config. Absent digest + hostname → system CA store. Absent
/// digest + bare IP → no validation (direct LAN/dev). Present digest → pinning.
#[cfg(not(target_family = "wasm"))]
fn native_client_config(url: &str, cert_digest: Option<[u8; 32]>) -> ClientConfig {
    use aeronet_webtransport::wtransport::{config::IpBindConfig, tls::Sha256Digest};
    use core::time::Duration;

    let config = ClientConfig::builder().with_bind_config(IpBindConfig::InAddrAnyV4);
    let config = if let Some(hash) = cert_digest {
        // Dev: self-signed cert pinned by its SHA-256 digest (explicit override).
        info!("[net] connecting to {url} with pinned cert digest");
        let digest = Sha256Digest::new(hash);
        config.with_server_certificate_hashes([digest])
    } else if url_host_is_bare_ip(url) {
        // Direct bare-IP dial, no digest: there's no DNS name to match a CA
        // cert's SAN, and an IP dial is a LAN/dev convenience against a
        // self-signed server. Skip validation entirely so it Just Works.
        // INSECURE (MITM-able) — use a hostname + CA cert for anything public.
        // Never reached for hostname URLs, which keep full CA validation below.
        warn!(
            "[net] connecting to {url} with NO cert validation (direct IP — insecure, LAN/dev only)"
        );
        config.with_no_cert_validation()
    } else {
        // Production: real CA cert on a domain (e.g. Let's Encrypt for
        // sandbox.lunco.space). Use the system root store — no digest needed.
        info!("[net] connecting to {url} with CA validation (no digest)");
        config.with_native_certs()
    };

    config
        .keep_alive_interval(Some(Duration::from_secs(1)))
        // 30s (was 5s): a client's frame loop legitimately stalls past a few
        // seconds during heavy startup (USD scene load + Modelica cosim compile)
        // or under host load, which stops keepalives and got the connection
        // dropped almost immediately. 30s tolerates those hitches while still
        // reaping a truly-dead peer. Must stay ≥ the server netcode client
        // timeout (see `NetcodeConfig` in server.rs) so neither layer races ahead.
        .max_idle_timeout(Some(Duration::from_secs(30)))
        .expect("valid idle timeout")
        .build()
}

/// Browser client config. Absent digest → normal CA validation. Present digest
/// → pin for a self-signed development certificate.
#[cfg(target_family = "wasm")]
fn wasm_client_config(cert_hash: Option<[u8; 32]>) -> ClientConfig {
    use aeronet_webtransport::xwt_web::{CertificateHash, HashAlgorithm};

    let server_certificate_hashes = if let Some(hash) = cert_hash {
        vec![CertificateHash {
            algorithm: HashAlgorithm::Sha256,
            value: Vec::from(hash),
        }]
    } else {
        Vec::new()
    };

    ClientConfig {
        server_certificate_hashes,
        ..Default::default()
    }
}

/// Decode one optional SHA-256 pin at connection admission. Only documented
/// colon/whitespace separators may be removed; other characters reject.
pub(crate) fn parse_certificate_digest(
    value: &str,
) -> core::result::Result<Option<[u8; 32]>, String> {
    if value.trim().is_empty() {
        return Ok(None);
    }
    let mut hash = [0u8; 32];
    let mut digits = 0usize;
    for character in value.chars() {
        if character == ':' || character.is_ascii_whitespace() {
            continue;
        }
        let nibble = character
            .to_digit(16)
            .ok_or_else(|| "certificate digest contains a non-hex character".to_string())?;
        if digits >= 64 {
            return Err("certificate digest must contain exactly 64 hex digits".into());
        }
        hash[digits / 2] |= (nibble as u8) << (if digits.is_multiple_of(2) { 4 } else { 0 });
        digits += 1;
    }
    if digits != 64 {
        return Err(format!(
            "certificate digest must contain exactly 64 hex digits, got {digits}"
        ));
    }
    Ok(Some(hash))
}

#[cfg(test)]
mod configuration_tests {
    use super::*;
    #[test]
    fn certificate_pin_rejects_invalid_explicit_input_and_preserves_valid_bytes() {
        assert_eq!(parse_certificate_digest(""), Ok(None));
        assert_eq!(parse_certificate_digest(" "), Ok(None));
        assert_eq!(
            parse_certificate_digest(&"AB:".repeat(32)),
            Ok(Some([0xab; 32]))
        );
        assert_eq!(
            parse_certificate_digest(&"ab".repeat(32)),
            Ok(Some([0xab; 32]))
        );
        for invalid in [
            ":",
            "g",
            "ab",
            &"a".repeat(63),
            &"a".repeat(65),
            &format!("{}g", "ab".repeat(32)),
        ] {
            assert!(parse_certificate_digest(invalid).is_err());
        }
    }
}
