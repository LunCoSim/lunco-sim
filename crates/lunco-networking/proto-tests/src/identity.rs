//! M1 — deterministic identity from provenance. Pure logic, no backend.
//!
//! Mirrors `IDENTITY.md`. The point of the tests around this module: prove that
//! two independent "processes" loading the same content derive the *same* id with
//! zero coordination, and that the derivation is a fixed, cross-platform-stable
//! function (NOT `DefaultHasher`).

/// 53-bit JS-safe identity space (same as the existing `make_id_53`).
pub const ID_MASK_53: u64 = (1u64 << 53) - 1;

/// Where an entity came from — the required input to identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Provenance {
    /// Instantiated from shared, content-addressed source. Deterministic id.
    Content {
        namespace: String,
        source: String,
        path: String,
    },
    /// Deterministic sub-part of a parent (rover→wheel, device→port).
    Derived { parent: u64, role: String },
    /// Born at runtime, not derivable. Server-allocated (no derived id here).
    Authoritative,
    /// Never networked.
    Local,
}

/// FNV-1a 64-bit. Fixed and identical on every platform — the whole point.
/// (A real impl would use blake3/xxhash; FNV-1a is enough to prove determinism
/// and keeps this crate dependency-free.)
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Fold 64 bits into the 53-bit space, mixing high bits down so we don't just
/// discard entropy.
fn fold_53(h: u64) -> u64 {
    (h ^ (h >> 53) ^ (h >> 32)) & ID_MASK_53
}

/// Canonicalize a content path so byte-identical inputs are guaranteed across
/// platforms: `\`→`/`, collapse `//`, drop trailing `/` (except root).
pub fn canonicalize_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut prev_slash = false;
    for ch in path.chars() {
        let c = if ch == '\\' { '/' } else { ch };
        if c == '/' {
            if prev_slash {
                continue;
            }
            prev_slash = true;
        } else {
            prev_slash = false;
        }
        out.push(c);
    }
    if out.len() > 1 && out.ends_with('/') {
        out.pop();
    }
    out
}

/// Derive the stable id, or `None` for provenance kinds that don't derive
/// (`Authoritative` is server-allocated; `Local` is never networked).
pub fn derive_id(p: &Provenance) -> Option<u64> {
    match p {
        Provenance::Content {
            namespace,
            source,
            path,
        } => {
            let mut buf = Vec::new();
            buf.extend_from_slice(namespace.as_bytes());
            buf.push(b':');
            buf.extend_from_slice(source.as_bytes());
            buf.push(b':');
            buf.extend_from_slice(canonicalize_path(path).as_bytes());
            Some(fold_53(fnv1a64(&buf)))
        }
        Provenance::Derived { parent, role } => {
            let mut buf = Vec::new();
            buf.extend_from_slice(&parent.to_le_bytes());
            buf.push(b'/');
            buf.extend_from_slice(role.as_bytes());
            Some(fold_53(fnv1a64(&buf)))
        }
        Provenance::Authoritative | Provenance::Local => None,
    }
}

/// Convenience constructor for content provenance.
pub fn content(namespace: &str, source: &str, path: &str) -> Provenance {
    Provenance::Content {
        namespace: namespace.into(),
        source: source.into(),
        path: path.into(),
    }
}
