//! Scenario distribution — the server publishes the scenario it's running,
//! connected clients fetch the assets they're missing, and a periodically
//! changing scenario propagates by revision bump.
//!
//! This module owns the **content-addressing + wire message shapes** for the
//! feature; the transport (chunking, flow control, OPFS persistence, scene
//! load) is layered on top in later phases. Phase 1 ships the manifest only —
//! enough for a host to tell a client "this is scenario X at revision R with
//! these asset CIDs", and for the client to stash it.
//!
//! # Content addressing — real IPLD CIDs
//!
//! Every asset is identified by a **CIDv1** with the `raw` codec (`0x55`) and a
//! `sha2-256` multihash, i.e. exactly what `ipfs add --raw-leaves --cid-version 1
//! <file>` produces for a single-block file. The CID is the IPFS-addressable
//! identity of that asset's bytes; `ipfs get <cid>` / `ipfs pin add <cid>` work
//! on the same content a LunCoSim server streams over WebTransport. Large files
//! that IPFS would chunk into 256 KiB UnixFS leaves still get a 1:1 raw-block CID
//! here — that's the *asset identity* (cache key + dedup), distinct from any
//! future "publish a UnixFS-wrapped directory to an IPFS gateway" step.
//!
//! On the wire a CID travels as its **canonical bytes** (`Cid::to_bytes()`,
//! 36 B for sha2-256 CIDv1) inside a `Vec<u8>` — bincode-friendly and
//! codec/hash-version-agnostic (a future blake3 or sha3 switch needs no wire
//! change). The [`cid`] crate is used only at build/parse/render boundaries.
//!
//! # Scenario identity + revision
//!
//! - **`scenario_id`** = the `twin.toml` `uuid` (`[u8; 16]` on the wire). Stable
//!   across renames/restarts once minted; says *which* scenario this is.
//! - **`revision`** = a Git-style Merkle root (`[u8; 32]`) over the sorted
//!   `(path → CID)` descriptor list. Says *which version* of it — it changes iff
//!   an asset's content (and thus its CID) or the set of paths changes. The
//!   client diffs `revision` against its cached scenario to decide "nothing to
//!   do" vs "fetch the changed CIDs". O(log n) incremental sync via per-path
//!   comparison of the asset list, not a rolling-hash rsync (assets are discrete
//!   content-addressed files — a "delta" is just "the asset whose CID differs").
//!
//! See `docs/architecture/13-twin-and-workflow.md` §3a (the aspirational
//! `[scenarios.*]` section) and `SYNC_ARCHITECTURE.md` §3 (the "Asset files →
//! content-addressed → fetch by hash → M1" row) for the design lineage.

use bevy::prelude::*;
use lunco_hash::content::Cid;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// IPLD codec for a raw byte block — the identity IPFS uses for `--raw-leaves`
/// single-block files. Re-exported from the shared hashing substrate so there is
/// one canonical definition (used by both this crate and the precompute cache).
pub use lunco_hash::content::RAW_CODEC;

/// Build the CIDv1 (`raw` codec + sha2-256) content address of `bytes` — the
/// IPFS-resolvable identity of an asset's content. Same bytes ⇒ same CID on
/// every peer, with zero coordination (the M1 contract). Thin domain alias over
/// [`lunco_hash::content::cid`].
pub fn cid_for_content(bytes: &[u8]) -> Cid {
    lunco_hash::content::cid(bytes)
}

/// Parse canonical CID bytes (as carried on the wire / stored in a manifest
/// sidecar) back into a [`Cid`]. Returns `None` on malformed input — callers
/// treat a bad CID as "asset unknown" rather than panicking the netcode.
pub fn cid_from_bytes(bytes: &[u8]) -> Option<Cid> {
    lunco_hash::content::cid_from_bytes(bytes)
}

/// One asset in a scenario manifest: its path relative to the scenario root,
/// its content-addressed identity (canonical CID bytes), its byte size, and an
/// optional media type (e.g. `"model/vnd.usd"`, `"model/gltf-binary"`,
/// `"image/png"` — an IPFS/Future-resolvability hint; not validated by the
/// netcode, used by the loader + for debug).
///
/// This is the OCI-image-descriptor shape `{path, digest, size, mediaType}`,
/// expressed with a real CID for the digest — standards-shaped and interops
/// with IPFS tooling (`ipfs pin add <cid>`).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScenarioAsset {
    /// Path relative to the scenario root, `/`-separated, no leading `/`.
    /// The client materialises assets at `<scenario_cache_dir>/<path>`.
    pub path: String,
    /// Canonical CIDv1 bytes (`Cid::to_bytes()`). 36 B for sha2-256. The
    /// client fetches by this identity and verifies the downloaded bytes hash
    /// back to the same CID.
    pub cid: Vec<u8>,
    /// Asset size in bytes. Lets the client show progress + pre-allocate, and
    /// lets a cache-hit check skip a hash recompute when size + CID match.
    pub size: u64,
    /// Optional media type hint (IPFS/IANA-style). Not load-bearing for sync.
    /// NB: no `skip_serializing_if` — this rides bincode (positional, not
    /// self-describing), so every field must always be emitted or the
    /// deserializer desyncs. `Option` is 1 tag byte + payload on the wire.
    pub media_type: Option<String>,
}

/// Compute the scenario **revision** — a Git-style Merkle root over the sorted
/// `(path, cid)` descriptor list. The client compares this `[u8; 32]` against
/// its cached scenario's revision to decide "nothing to do" vs "sync".
///
/// Canonical encoding (deterministic across peers): sort assets by `path`,
/// then for each append `varint(path.len()) || path || cid_bytes`. The
/// `varint` length prefix makes the encoding unambiguous (no path can be a
/// prefix of another and collide). SHA-256 of the concatenation = revision.
/// Changes iff an asset's CID, a path, or the set of paths changes.
pub fn scenario_revision(assets: &[ScenarioAsset]) -> [u8; 32] {
    let mut sorted: Vec<&ScenarioAsset> = assets.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    let mut hasher = Sha256::new();
    for a in &sorted {
        let path_bytes = a.path.as_bytes();
        hasher.update(varint(path_bytes.len() as u64));
        hasher.update(path_bytes);
        hasher.update(&a.cid);
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&hasher.finalize());
    out
}

/// Minimal unsigned-varint encoder (LEB128) for the revision's length
/// prefixes. Kept local — we don't pull `unsigned-varint` as a workspace dep
/// just for this; the encoding is 7-bits-per-byte with a continuation bit.
fn varint(mut n: u64) -> [u8; 10] {
    let mut buf = [0u8; 10];
    let mut i = 0;
    loop {
        let b = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            buf[i] = b;
            break;
        }
        buf[i] = b | 0x80;
        i += 1;
    }
    buf
}

// ── Wire messages ─────────────────────────────────────────────────────────────

/// Host → client: "the scenario I'm running is `scenario_id` at `revision`,
/// with these assets. Fetch the ones you're missing (Phase 3) and load the
/// scene at `default_scene` once you have everything." Rides the reliable
/// `CmdChannel` alongside Handshake/Ownership/Profiles — it's session context,
/// not per-tick state. Small (asset list of CIDs, no bytes); the asset *bytes*
/// arrive via [`AssetChunkMsg`] in Phase 3.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ScenarioManifestMsg {
    /// Stable cross-session scenario identity, 16 bytes. Normally the
    /// `twin.toml` uuid (`Uuid::as_bytes()`); for an unmanaged folder (no
    /// `twin.toml`) the **host** derives it as a SHA-256 path digest
    /// (truncated to 16 B) so distinct folder-scenarios get distinct ids
    /// instead of colliding on all-zeros. Either way the client keys its
    /// `scenarios/<scenario_id>/` asset cache on this value.
    pub scenario_id: [u8; 16],
    /// Git-style Merkle root of the asset list — `scenario_revision()`. The
    /// client diffs this against its cached revision to short-circuit a re-fetch.
    pub revision: [u8; 32],
    /// Human-readable scenario name (from `twin.toml` `name`). For UI + debug.
    pub name: String,
    /// Optional entry-point scene path relative to the scenario root (from
    /// `[usd] default_scene`). `None` = no USD scene to auto-load. No
    /// `skip_serializing_if`: bincode is positional (see `media_type`).
    pub default_scene: Option<String>,
    /// The scenario's assets, descriptor-only (path + CID + size + media type).
    /// No bytes — those come via `AssetChunkMsg` in Phase 3.
    pub assets: Vec<ScenarioAsset>,
    /// The host's Twin-journal head at the moment this manifest was built — the
    /// **base** the downloaded asset snapshot corresponds to. The client loads
    /// files at this state, then the journal plane replays only entries AFTER
    /// this head onto the scene (Layer B), so host edits made *after* the build
    /// appear without double-applying the history already baked into the files.
    /// `None` if the host has no journal / an empty history. (bincode-safe:
    /// `EntryId` is `{author: String, lamport: u64}`, no `serde_json::Value`.)
    pub journal_head: Option<lunco_twin_journal::EntryId>,
    /// Where to fetch asset bytes over **HTTP**, e.g. `http://10.0.0.5:5889/assets/`
    /// or (behind an nginx proxy, same-origin for a wasm client) `/assets/`. A CID
    /// is appended verbatim: `<base><cid-base32>`.
    ///
    /// The bytes plane rides HTTP, not the QUIC game port: lightyear's reliable
    /// sender queues without bound (`buffer_send` never rejects), so streaming a
    /// multi-MB twin through `AssetChunkMsg` saturates the link and stalls the
    /// session. HTTP gives us the OS's own flow control for free. `None` = the host
    /// serves no asset endpoint; the client falls back to the QUIC chunk path,
    /// which is correct but only viable for small scenarios.
    ///
    /// Appended last: bincode is positional, so new fields must not shift the
    /// existing ones (see `media_type`).
    pub asset_base_url: Option<String>,
}

/// Client → host: "I'm missing these assets — send me their bytes." Each entry
/// is canonical CID bytes. The host responds with a stream of
/// [`AssetChunkMsg`]s per requested CID. Phase 3 wires the request emission +
/// host chunker; the variant is reserved on the wire now so a stale wasm client
/// vs fresh host doesn't break when Phase 3 lands (positional bincode).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AssetRequestMsg {
    /// CIDs the client needs, as canonical bytes. Empty = client has
    /// everything (a "no-op" request the host can ignore).
    pub missing: Vec<Vec<u8>>,
}

/// Host → client: one chunk of an asset's bytes, addressed by CID. Reassembled
/// in order (`offset`), verified by hashing back to the CID, then persisted via
/// `lunco_storage::write_file_sync`. Phase 3 owns the chunker/reassembler; the
/// variant is reserved on the wire now.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AssetChunkMsg {
    /// Canonical CID bytes of the asset this chunk belongs to.
    pub cid: Vec<u8>,
    /// Byte offset within the asset where this chunk starts.
    pub offset: u64,
    /// Total asset size in bytes (so the client knows completion + can
    /// pre-allocate). Repeated per chunk for stateless reassembly.
    pub total: u64,
    /// The chunk payload. Sized to stay well under the lightyear fragment limit
    /// (Phase 3 picks the exact cap; ~64 KiB).
    pub data: Vec<u8>,
}

/// Peer → host: "I already have this CID cached" — a dedupe/late-join hint so
/// the host can skip streaming an asset the peer already owns (e.g. a re-
/// connecting client, or a shared asset across scenarios). Phase 3+; reserved
/// on the wire now.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AssetHaveMsg {
    /// Canonical CID bytes the peer already has.
    pub cid: Vec<u8>,
}

/// Peer → host: an asset a client **imported** into the shared twin. The host
/// verifies `data` against `cid` (fail-closed), writes it into its twin at `path`,
/// and rebuilds + re-advertises the manifest — so a client's import distributes to
/// every peer through the existing host→client fetch (`AssetChunkMsg`). This is the
/// bidirectional-ingest counterpart of the host-authoritative serve: "connect to a
/// server → import something → it distributes."
///
/// TODO(bidirectional-content): (1) chunk large offers like [`AssetChunkMsg`] — this
/// carries whole bytes, fine for scripts / small assets (the caller caps size);
/// (2) Option B — make the manifest a **journaled document** + drop the host-only
/// serve gate so any CID holder serves, and an import needs no host round-trip
/// (the content plane becomes as symmetric as the journal plane).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AssetOfferMsg {
    /// Twin-relative path the asset should live at (e.g. `"imports/foo.glb"`).
    pub path: String,
    /// Canonical CID bytes — the host verifies `data` against this before writing.
    pub cid: Vec<u8>,
    /// The asset bytes.
    pub data: Vec<u8>,
}

// ── Resources ─────────────────────────────────────────────────────────────────

/// Host-side: the scenario this server is currently running. Built by the app
/// (`lunco-sandbox`'s `setup_sandbox`) after the Twin is opened — it walks the
/// Twin's files, hashes each, and fills [`Self::manifest`]. The
/// [`on_server_connected`](crate::server) observer sends it to each new client
/// and [`broadcast_scenario_manifest`] pushes it to all clients when the
/// `revision` changes (the host re-loaded the scenario).
///
/// `None`-defaulted: a host that hasn't loaded a scenario (bare server) sends
/// no manifest; clients stay on their local scene. The host's
/// `on_server_connected` arm reads `Option<Res<ScenarioManifestResource>>` so
/// the system registers even before a scenario is loaded.
#[derive(Resource, Default, Clone, Debug)]
pub struct ScenarioManifestResource {
    /// The current scenario manifest. `None` until the host opens a Twin/scene.
    pub manifest: Option<ScenarioManifestMsg>,
}

/// Client-side: the latest scenario manifest received from the host. Stashed by
/// the `ScenarioManifest` arm of `drain_sync_inbox`. Phase 3 reads this to emit
/// `AssetRequestMsg` for the CIDs missing from the local cache; Phase 4 loads
/// the scene once all assets are present. `None` until the host sends one.
#[derive(Resource, Default, Clone, Debug)]
pub struct RemoteScenarioManifest {
    /// The most recent manifest the host pushed. Replaced on each
    /// `ScenarioManifest` envelope (the host's revision is monotonic per
    /// scenario; a different `scenario_id` is a full scenario swap).
    pub manifest: Option<ScenarioManifestMsg>,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cid_for_content_is_deterministic_and_ipfs_shaped() {
        // Same bytes ⇒ same CID, every peer.
        let a = cid_for_content(b"hello scenario");
        let b = cid_for_content(b"hello scenario");
        assert_eq!(a.to_bytes(), b.to_bytes());
        // Different bytes ⇒ different CID.
        let c = cid_for_content(b"hello scenario!");
        assert_ne!(a.to_bytes(), c.to_bytes());
        // CIDv1 + raw codec + sha2-256 ⇒ 36 canonical bytes
        // (1B version + 1B codec + 2B multihash code/len + 32B digest).
        assert_eq!(a.version(), lunco_hash::content::Version::V1);
        assert_eq!(a.codec(), RAW_CODEC);
        assert_eq!(a.to_bytes().len(), 36);
    }

    #[test]
    fn cid_round_trips_through_bytes() {
        let cid = cid_for_content(b"asset bytes");
        let bytes = cid.to_bytes();
        let back = cid_from_bytes(&bytes).expect("parse");
        assert_eq!(back.to_bytes(), bytes);
    }

    #[test]
    fn cid_renders_as_ipfs_base32_string() {
        // The string form is what `ipfs pin add <cid>` accepts. A CIDv1 with
        // the `raw` codec (0x55) + sha2-256 base32-renders as `bafkrei…`
        // (`bafybei…`/`bafy…` is the dag-pb codec — what plain `ipfs add`
        // gives; we use raw single-block leaves, cf. `--raw-leaves`).
        let cid = cid_for_content(b"interop check");
        let s = cid.to_string();
        assert!(s.starts_with("bafk"), "raw-codec CIDv1 base32 starts with bafk…, got {s}");
        // Round-trips through the string too.
        let parsed: Cid = s.parse().expect("parse cid string");
        assert_eq!(parsed.to_bytes(), cid.to_bytes());
    }

    #[test]
    fn scenario_revision_is_stable_under_reorder() {
        let assets = vec![
            ScenarioAsset {
                path: "scenes/main.usda".into(),
                cid: cid_for_content(b"scene").to_bytes(),
                size: 5,
                media_type: Some("model/vnd.usd".into()),
            },
            ScenarioAsset {
                path: "assets/rover.glb".into(),
                cid: cid_for_content(b"rover").to_bytes(),
                size: 5,
                media_type: None,
            },
        ];
        let rev_sorted = scenario_revision(&assets);
        // Same assets, different declaration order ⇒ same revision (sorts internally).
        let mut shuffled = assets.clone();
        shuffled.reverse();
        let rev_shuffled = scenario_revision(&shuffled);
        assert_eq!(rev_sorted, rev_shuffled);
    }

    #[test]
    fn scenario_revision_changes_on_content_change() {
        let a = ScenarioAsset {
            path: "x".into(),
            cid: cid_for_content(b"v1").to_bytes(),
            size: 2,
            media_type: None,
        };
        let b = ScenarioAsset {
            path: "x".into(),
            cid: cid_for_content(b"v2").to_bytes(), // different content
            size: 2,
            media_type: None,
        };
        assert_ne!(scenario_revision(&[a]), scenario_revision(&[b]));
    }

    #[test]
    fn scenario_revision_changes_on_path_set_change() {
        let one = vec![ScenarioAsset {
            path: "a".into(),
            cid: cid_for_content(b"x").to_bytes(),
            size: 1,
            media_type: None,
        }];
        let two = vec![
            ScenarioAsset {
                path: "a".into(),
                cid: cid_for_content(b"x").to_bytes(),
                size: 1,
                media_type: None,
            },
            ScenarioAsset {
                path: "b".into(),
                cid: cid_for_content(b"y").to_bytes(),
                size: 1,
                media_type: None,
            },
        ];
        assert_ne!(scenario_revision(&one), scenario_revision(&two));
    }

    #[test]
    fn varint_encodes_leb128() {
        assert_eq!(&varint(0)[..1], &[0x00]);
        assert_eq!(&varint(127)[..1], &[0x7f]);
        assert_eq!(&varint(128)[..2], &[0x80, 0x01]);
        assert_eq!(&varint(255)[..2], &[0xff, 0x01]);
        assert_eq!(&varint(300)[..2], &[0xac, 0x02]);
    }
}
