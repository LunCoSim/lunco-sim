//! Browser-only WebTransport client IO that dials a **hostname URL**.
//!
//! lightyear's built-in `WebTransportClientIo` builds its dial URL as
//! `https://{SocketAddr}` (IP-only) from a `PeerAddr` — so a real CA cert issued
//! for `lunica.lunco.space` can never validate (the URL host is an IP). This is
//! a thin re-implementation of lightyear's link observer that dials a URL **we**
//! control (`https://lunica.lunco.space:5888`), letting the browser resolve the
//! name and validate the domain cert with **no digest**. A self-signed dev cert
//! still pins via `certificate_digest` (empty ⇒ normal CA validation).
//!
//! We deliberately do **not** add `AeronetPlugin` / aeronet's
//! `WebTransportClientPlugin` here — lightyear's `ClientPlugins` already adds
//! them (the `webtransport` feature). We only register our own `link` observer,
//! which fires for entities carrying [`WtUrlClientIo`] (lightyear's fires for
//! `WebTransportClientIo`; the two coexist).

use aeronet_webtransport::client::{ClientConfig, WebTransportClient};
use bevy::prelude::*;
use lightyear::prelude::{LinkStart, Linked, Linking};
use lightyear_aeronet::AeronetLinkOf;

/// Component on the client entity: the full WebTransport URL to dial plus the
/// optional self-signed cert digest. An **empty** digest means no
/// `serverCertificateHashes` ⇒ the browser does normal CA validation (the
/// production path with a real cert on a domain). A hex digest pins a specific
/// self-signed cert (localhost dev).
#[derive(Component)]
pub(crate) struct WtUrlClientIo {
    /// e.g. `https://lunica.lunco.space:5888`.
    pub url: String,
    /// Bare lowercase hex SHA-256 of a self-signed cert, or empty for CA.
    pub certificate_digest: String,
}

/// Registers the URL-dialing link observer. Add once; lightyear's WebTransport
/// plugin (already pulled in by `ClientPlugins`) owns the aeronet session setup.
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
            let config = client_config(digest)?;
            let entity_mut = world.spawn((AeronetLinkOf(entity), Name::from("WtUrlClient")));
            // On wasm the connect target is the URL string directly.
            WebTransportClient::connect(config, url).apply(entity_mut);
            Ok(())
        });
    }
    Ok(())
}

/// Build the browser `ClientConfig`. Empty digest ⇒ no `serverCertificateHashes`
/// ⇒ normal CA validation. Mirrors `lightyear_webtransport`'s wasm `client_config`.
fn client_config(cert_hash: String) -> Result<ClientConfig> {
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
