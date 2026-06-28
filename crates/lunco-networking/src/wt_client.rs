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
/// optional self-signed cert digest. An **empty** digest means:
/// - browser: no `serverCertificateHashes` → normal CA validation.
/// - native: uses the system CA store → normal CA validation.
///
/// A non-empty hex digest pins a specific self-signed cert (localhost dev only).
#[derive(Component)]
pub(crate) struct WtUrlClientIo {
    /// Full URL, e.g. `https://sandbox.lunco.space:5888`.
    pub url: String,
    /// Bare lowercase hex SHA-256 of a self-signed cert, or empty for CA.
    pub certificate_digest: String,
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
) -> Result {
    if let Ok((entity, io)) = query.get(trigger.entity) {
        let url = io.url.clone();
        let digest = io.certificate_digest.clone();
        commands.queue(move |world: &mut World| -> Result {
            let config = client_config(&url, digest)?;
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
            Ok(())
        });
    }
    Ok(())
}

/// Build the `ClientConfig` for the given URL.
///
/// - Empty digest: use system CA store (native) / no `serverCertificateHashes`
///   (browser) → normal CA chain validation. **Production path.**
/// - Non-empty hex digest: pin a specific self-signed cert SHA-256.
///   **Dev/localhost only.**
fn client_config(url: &str, cert_hash: String) -> Result<ClientConfig> {
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

/// Native client config. Empty digest + hostname → system CA store (validates
/// `sandbox.lunco.space`'s Let's Encrypt cert normally). Empty digest + bare IP
/// → no validation (direct LAN/dev). Non-empty digest → self-signed cert pinning.
#[cfg(not(target_family = "wasm"))]
fn native_client_config(url: &str, cert_digest: String) -> Result<ClientConfig> {
    use aeronet_webtransport::wtransport::{config::IpBindConfig, tls::Sha256Digest};
    use core::time::Duration;

    let config = ClientConfig::builder().with_bind_config(IpBindConfig::InAddrAnyV4);
    let config = if !cert_digest.is_empty() {
        // Dev: self-signed cert pinned by its SHA-256 digest (explicit override).
        info!("[net] connecting to {url} with pinned cert digest");
        let bytes = from_hex(&cert_digest)?;
        // A SHA-256 digest is exactly 32 bytes; `from_hex` only guarantees even
        // length, so guard before `copy_from_slice` (which panics on a length
        // mismatch) and fail gracefully on a malformed `LUNCO_CERT_DIGEST`.
        if bytes.len() != 32 {
            return Err(format!(
                "cert digest must be 32 bytes (64 hex chars), got {} bytes: {cert_digest}",
                bytes.len()
            )
            .into());
        }
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&bytes);
        let digest = Sha256Digest::new(hash);
        config.with_server_certificate_hashes([digest])
    } else if url_host_is_bare_ip(url) {
        // Direct bare-IP dial, no digest: there's no DNS name to match a CA
        // cert's SAN, and an IP dial is a LAN/dev convenience against a
        // self-signed server. Skip validation entirely so it Just Works.
        // INSECURE (MITM-able) — use a hostname + CA cert for anything public.
        // Never reached for hostname URLs, which keep full CA validation below.
        warn!("[net] connecting to {url} with NO cert validation (direct IP — insecure, LAN/dev only)");
        config.with_no_cert_validation()
    } else {
        // Production: real CA cert on a domain (e.g. Let's Encrypt for
        // sandbox.lunco.space). Use the system root store — no digest needed.
        info!("[net] connecting to {url} with CA validation (no digest)");
        config.with_native_certs()
    };

    Ok(config
        .keep_alive_interval(Some(Duration::from_secs(1)))
        // 30s (was 5s): a client's frame loop legitimately stalls past a few
        // seconds during heavy startup (USD scene load + Modelica cosim compile)
        // or under host load, which stops keepalives and got the connection
        // dropped almost immediately. 30s tolerates those hitches while still
        // reaping a truly-dead peer. Must stay ≥ the server netcode client
        // timeout (see `NetcodeConfig` in server.rs) so neither layer races ahead.
        .max_idle_timeout(Some(Duration::from_secs(30)))
        .expect("valid idle timeout")
        .build())
}

/// Browser client config. Empty digest → no `serverCertificateHashes` → normal
/// CA validation. Non-empty → pin for localhost self-signed dev cert.
#[cfg(target_family = "wasm")]
fn wasm_client_config(cert_hash: String) -> Result<ClientConfig> {
    use aeronet_webtransport::xwt_web::{CertificateHash, HashAlgorithm};

    let server_certificate_hashes = if cert_hash.is_empty() {
        Vec::new()
    } else {
        let hash = from_hex(&cert_hash)?;
        vec![CertificateHash {
            algorithm: HashAlgorithm::Sha256,
            value: Vec::from(hash),
        }]
    };

    Ok(ClientConfig {
        server_certificate_hashes,
        ..Default::default()
    })
}

// Hex → bytes for the cert digest (bare lowercase hex, no colons). Adapted from
// lightyear_webtransport, which adapted it from ring's test helpers.
fn from_hex(hex_str: &str) -> core::result::Result<Vec<u8>, String> {
    if !hex_str.len().is_multiple_of(2) {
        return Err(format!(
            "cert digest hex has an odd number of digits ({}): {hex_str}",
            hex_str.len(),
        ));
    }
    let mut result = Vec::with_capacity(hex_str.len() / 2);
    for digits in hex_str.as_bytes().chunks(2) {
        let hi = from_hex_digit(digits[0])?;
        let lo = from_hex_digit(digits[1])?;
        result.push((hi * 0x10) | lo);
    }
    Ok(result)
}

fn from_hex_digit(d: u8) -> core::result::Result<u8, String> {
    use core::ops::RangeInclusive;
    const DECIMAL: (u8, RangeInclusive<u8>) = (0, b'0'..=b'9');
    const HEX_LOWER: (u8, RangeInclusive<u8>) = (10, b'a'..=b'f');
    const HEX_UPPER: (u8, RangeInclusive<u8>) = (10, b'A'..=b'F');
    for (offset, range) in &[DECIMAL, HEX_LOWER, HEX_UPPER] {
        if range.contains(&d) {
            return Ok(d - range.start() + offset);
        }
    }
    Err(format!("invalid hex digit '{}'", d as char))
}
