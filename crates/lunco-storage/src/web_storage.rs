//! Browser `localStorage` backend for wasm builds — the web-side
//! counterpart to [`crate::FileStorage`].
//!
//! `FileStorage`'s `std::fs` paths are `#[cfg(not(wasm32))]`-guarded, so
//! on wasm a `File`/`Memory` handle would otherwise fall through to
//! [`StorageError::Unsupported`]. `WebStorage` gives the wasm build real
//! persistence by mapping each handle onto a `localStorage` key:
//!
//!   * [`StorageHandle::File`]   → key `lunco-fs:<path>`
//!   * [`StorageHandle::Memory`] → key `lunco-mem:<key>`
//!
//! Bytes are hex-encoded because `localStorage` only stores DOMStrings.
//! That doubles the on-disk size, so this backend is intended for the
//! document-sized payloads the workbench actually persists (Modelica
//! sources, `.usda` stages, small JSON). Large binary assets (glTF,
//! textures) should move to an OPFS backend once `opfs_stub` lands —
//! see the `StorageHandle::Opfs` variant in `lib.rs`.
//!
//! Pickers are not meaningful in a sandboxed browser origin (there is no
//! ambient filesystem); they return [`StorageError::Unsupported`]. The
//! wasm UI drives open/save through the FSA picker in `lunco-workbench`
//! instead, which yields explicit handles.
//!
//! The `_ =>` arms below land the feature-gated `Fsa`/`Idb`/`Opfs`/`Http`
//! variants; with the default feature set they're unreachable, so silence
//! the lint file-wide (same as `file_storage.rs`) rather than cfg-splitting
//! every match.
#![allow(unreachable_patterns)]

use async_trait::async_trait;

use crate::{Storage, StorageError, StorageHandle, StorageResult};

/// Browser-`localStorage` backend. Zero-sized — all state lives in the
/// origin's `localStorage`, shared across every `WebStorage` instance
/// (unlike `FileStorage::Memory`, which is per-instance).
#[derive(Default)]
pub struct WebStorage;

impl WebStorage {
    /// Construct a fresh backend handle.
    pub fn new() -> Self {
        Self
    }

    /// Grab the origin's `localStorage`, mapping the two failure modes
    /// (no `window`, storage disabled) onto a single `Unsupported`.
    fn local_storage() -> Result<web_sys::Storage, StorageError> {
        web_sys::window()
            .and_then(|w| w.local_storage().ok().flatten())
            .ok_or_else(|| {
                StorageError::Unsupported("localStorage unavailable in this context".into())
            })
    }

    /// Derive the `localStorage` key for a handle. Only `File` / `Memory`
    /// are addressable on the web today; the remote/FSA variants are
    /// handled by their own backends.
    fn key(handle: &StorageHandle) -> Result<String, StorageError> {
        match handle {
            StorageHandle::File(path) => Ok(format!("lunco-fs:{}", path.display())),
            StorageHandle::Memory(k) => Ok(format!("lunco-mem:{k}")),
            _ => Err(StorageError::Unsupported(
                "WebStorage addresses File / Memory handles only".into(),
            )),
        }
    }
}

#[async_trait]
impl Storage for WebStorage {
    async fn read(&self, handle: &StorageHandle) -> StorageResult<Vec<u8>> {
        let key = Self::key(handle)?;
        let ls = Self::local_storage()?;
        match ls.get_item(&key).ok().flatten() {
            Some(hex) => from_hex(&hex),
            None => Err(StorageError::NotFound),
        }
    }

    async fn write(&self, handle: &StorageHandle, bytes: &[u8]) -> StorageResult<()> {
        let key = Self::key(handle)?;
        let ls = Self::local_storage()?;
        ls.set_item(&key, &to_hex(bytes)).map_err(|_| {
            // The common failure is `QuotaExceededError`; localStorage
            // gives us no structured error, so surface a generic I/O.
            StorageError::Io(std::io::Error::other("localStorage write failed (quota?)"))
        })
    }

    async fn delete(&self, handle: &StorageHandle) -> StorageResult<()> {
        let key = Self::key(handle)?;
        let ls = Self::local_storage()?;
        if ls.get_item(&key).ok().flatten().is_none() {
            return Err(StorageError::NotFound);
        }
        ls.remove_item(&key).map_err(|_| {
            StorageError::Io(std::io::Error::other("localStorage remove failed"))
        })
    }

    async fn exists(&self, handle: &StorageHandle) -> bool {
        let Ok(key) = Self::key(handle) else {
            return false;
        };
        Self::local_storage()
            .ok()
            .and_then(|ls| ls.get_item(&key).ok().flatten())
            .is_some()
    }

    async fn is_writable(&self, handle: &StorageHandle) -> bool {
        // Anything we can address is writable until the quota is hit
        // (which a real `write` reports). Unaddressable handles are not.
        Self::key(handle).is_ok()
    }
}

/// Lower-case hex encoding. `localStorage` only holds DOMStrings, so
/// arbitrary bytes have to be stringified; hex is round-trip-safe and
/// dependency-free (base64 would need a crate).
fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 0x0f) as u32, 16).unwrap());
    }
    s
}

/// Inverse of [`to_hex`]. Treats odd length or non-hex digits as a
/// corrupt record rather than a missing one.
fn from_hex(s: &str) -> StorageResult<Vec<u8>> {
    let bytes = s.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return Err(StorageError::Io(std::io::Error::other(
            "corrupt localStorage record (odd hex length)",
        )));
    }
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let hi = (pair[0] as char).to_digit(16);
        let lo = (pair[1] as char).to_digit(16);
        match (hi, lo) {
            (Some(h), Some(l)) => out.push(((h << 4) | l) as u8),
            _ => {
                return Err(StorageError::Io(std::io::Error::other(
                    "corrupt localStorage record (non-hex digit)",
                )))
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Codec tests run on any target — no browser needed.
    #[test]
    fn hex_roundtrip() {
        let cases: &[&[u8]] = &[b"", b"hello", &[0x00, 0xff, 0x10, 0xab]];
        for c in cases {
            assert_eq!(from_hex(&to_hex(c)).unwrap(), *c);
        }
    }

    #[test]
    fn hex_rejects_odd_and_nonhex() {
        assert!(from_hex("abc").is_err());
        assert!(from_hex("zz").is_err());
    }
}
