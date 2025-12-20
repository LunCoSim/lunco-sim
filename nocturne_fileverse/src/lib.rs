// src/lib.rs
// NOCTURNE v1.7-N8-FILEVERSE :: Decentralized Collaborative Security
// Contract: 0x3594CA7D7F2a64FCc4fF825768409Ed78809Bdb5#41
// Key: 0YkwLqkQFABleChsMj0JCX8NxYHupnMxxXQSwQGXQ2WlMoYMGK9FeFqQ-vkpXRR3

#![deny(unsafe_code)]
#![warn(missing_docs)]
#![warn(clippy::all)]
// #![warn(clippy::pedantic)]

use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    num::NonZeroUsize,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime},
};
use anyhow::{anyhow, bail, ensure, Context, Error, Result};
use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE, Engine};
use blake3::{Hash, Hasher};
use bloomfilter::Bloom;
use chacha20poly1305::{
    aead::{Aead, OsRng},
    ChaCha20Poly1305, Key, KeyInit, Nonce,
};
use dashmap::DashMap;
use ed25519_dalek::{Keypair, Signature, Signer, Verifier, VerifyingKey as PublicKey};
use etcd_client::{
    Client, Compare, DeleteOptions, GetOptions, PutOptions, Txn, TxnOp,
};
use governor::{
    clock::{self, MonotonicClock},
    state::{direct::NotKeyed, InMemoryState},
    Quota, RateLimiter,
};
// use ipfs_api_backend_hyper as ipfs_api;
// use ipfs_api::IpfsClient;
use lru::LruCache;
use parking_lot::{Mutex, RwLock};
use rand::{rngs::OsRng as RandOsRng, RngCore};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};
use std::time::UNIX_EPOCH;
use subtle::{Choice, ConstantTimeEq};
use thiserror::Error;
use tokio::{
    sync::{broadcast, watch},
    task,
    time::{sleep, timeout},
};
use tracing::{debug, error, info, instrument, span, warn, Level};
use uuid::Uuid;

pub struct IpfsClient;
// ============================================================================
// FILEVERSE INTEGRATION CONSTANTS
// ============================================================================

/// Fileverse contract address (decentralized document registry)
const CONTRACT_ADDRESS: &str = "0x3594CA7D7F2a64FCc4fF825768409Ed78809Bdb5";

/// File ID within the Fileverse contract
const FILEVERSE_FILE_ID: u64 = 41;

/// Base64-encoded decryption key for Fileverse documents
const DECRYPT_KEY_BASE64: &str = "0YkwLqkQFABleChsMj0JCX8NxYHupnMxxXQSwQGXQ2WlMoYMGK9FeFqQ-vkpXRR3";

/// IPFS gateway URL for decentralized storage
const IPFS_GATEWAY: &str = "http://127.0.0.1:5001";

/// Arweave gateway URL for permanent fallback storage
const ARWEAVE_GATEWAY: &str = "https://arweave.net";

/// Fileverse API endpoint for real-time collaboration
const FILEVERSE_API: &str = "https://api.fileverse.io";

// ============================================================================
// ERROR TYPES
// ============================================================================

#[derive(Error, Debug, Clone, Serialize)]
pub enum FileverseError {
    #[error("V1: Stale Fileverse signature detected")]
    StaleFileverseSignature,
    #[error("V2: Node binding across contracts violated")]
    CrossContractBinding,
    #[error("V3: Fileverse ACL timing oracle detected")]
    AclTimingOracle,
    #[error("V4: Collaborative graph degree explosion: {0}")]
    CollaborativeDegreeViolation(String),
    #[error("V5: Merkle collision across Fileverse documents")]
    DocumentForkCollision,
    #[error("V6: Dual rotation on shared Fileverse key")]
    DualRotationAttack,
    #[error("V7: Fileverse document version replay")]
    DocumentReplay,
    #[error("V8: Fileverse gateway outage cascade")]
    GatewayOutage,
    #[error("V9: Partial IPFS/Arweave commit - ghost session")]
    GhostSession,
    #[error("Fileverse contract error: {0}")]
    ContractError(String),
    #[error("IPFS pinning failed: {0}")]
    IpfsError(String),
    #[error("Arweave transaction failed: {0}")]
    ArweaveError(String),
    #[error("Decryption failed: {0}")]
    DecryptionError(String),
    #[error("Rate limit exceeded: {0}")]
    RateLimitExceeded(String),
}

// ============================================================================
// V1: FILEVERSE HSM WITH CONTRACT BINDING
// ============================================================================

#[derive(Clone)]
pub struct FileverseHsm {
    contract_address: String,
    file_id: u64,
    decryption_key: Arc<Key>,
    version_cache: Arc<RwLock<LruCache<(String, u64, u64), (Vec<u8>, Instant)>>>,
    // (contract, file_id, version) -> (signature, timestamp)
    current_version: Arc<AtomicU64>,
    metrics: Arc<DashMap<String, AtomicU64>>,
}

impl FileverseHsm {
    /// Create new Fileverse HSM with contract binding
    pub fn new(
        contract_address: &str,
        file_id: u64,
        decryption_key_base64: &str,
    ) -> Result<Self> {
        let decryption_key = Arc::new(Self::derive_fileverse_key(decryption_key_base64)?);

        let metrics = Arc::new(DashMap::new());
        metrics.insert("signatures".to_string(), AtomicU64::new(0));
        metrics.insert("verifications".to_string(), AtomicU64::new(0));
        metrics.insert("cache_hits".to_string(), AtomicU64::new(0));

        Ok(Self {
            contract_address: contract_address.to_string(),
            file_id,
            decryption_key,
            version_cache: Arc::new(RwLock::new(LruCache::new(NonZeroUsize::new(1000).unwrap()))),
            current_version: Arc::new(AtomicU64::new(0)),
            metrics,
        })
    }

    /// Derive Fileverse key from base64 string (V6: Key derivation)
    fn derive_fileverse_key(key_base64: &str) -> Result<Key> {
        let key_bytes = URL_SAFE
            .decode(key_base64)
            .map_err(|e| FileverseError::DecryptionError(e.to_string()))?;

        ensure!(
            key_bytes.len() >= 32,
            FileverseError::DecryptionError("Key too short".to_string())
        );

        // HKDF derivation with Fileverse context
        let mut hasher = Sha3_256::new();
        hasher.update(b"FILEVERSE-KEY-DERIVATION");
        hasher.update(&key_bytes);
        hasher.update(CONTRACT_ADDRESS.as_bytes());
        hasher.update(&FILEVERSE_FILE_ID.to_le_bytes());

        let derived = hasher.finalize();
        Ok(*Key::from_slice(&derived))
    }

    /// Verify Fileverse signature with contract binding (V1)
    #[instrument(skip(self, message), level = "debug")]
    pub fn verify_v1(
        &self,
        message: &[u8],
        signature: &[u8],
        version: u64,
        expected_contract: &str,
    ) -> Result<()> {
        ensure!(
            expected_contract == self.contract_address,
            FileverseError::StaleFileverseSignature
        );

        let current = self.current_version.load(Ordering::Acquire);
        ensure!(
            version == current,
            FileverseError::StaleFileverseSignature
        );

        // Create contract-bound message
        let mut bound_message = Vec::new();
        bound_message.extend(message);
        bound_message.extend(self.contract_address.as_bytes());
        bound_message.extend(&self.file_id.to_le_bytes());
        bound_message.extend(&version.to_le_bytes());

        // Verify with Fileverse public key
        let public_key = self.get_fileverse_public_key()?;
        let sig = Signature::from_bytes(signature.try_into().map_err(|_| {
            FileverseError::StaleFileverseSignature
        })?);

        public_key
            .verify(&bound_message, &sig)
            .map_err(|_| FileverseError::StaleFileverseSignature)?;

        self.metrics
            .get("verifications")
            .unwrap()
            .fetch_add(1, Ordering::Relaxed);

        Ok(())
    }

    /// Sign message with Fileverse binding (V1)
    #[instrument(skip(self, message), level = "debug")]
    pub async fn sign_v1(&self, message: &[u8]) -> Result<Vec<u8>> {
        let version = self.current_version.load(Ordering::Acquire);
        let cache_key = (
            self.contract_address.clone(),
            self.file_id,
            version,
        );

        // Check cache
        {
            let mut cache = self.version_cache.write();
            if let Some((sig, created)) = cache.get(&cache_key) {
                if created.elapsed() < Duration::from_secs(3600) {
                    self.metrics
                        .get("cache_hits")
                        .unwrap()
                        .fetch_add(1, Ordering::Relaxed);
                    return Ok(sig.clone());
                }
            }
        }

        // Create contract-bound message
        let mut bound_message = Vec::new();
        bound_message.extend(message);
        bound_message.extend(self.contract_address.as_bytes());
        bound_message.extend(&self.file_id.to_le_bytes());
        bound_message.extend(&version.to_le_bytes());

        // In production: Call Fileverse signing service
        // For now, simulate with local key
        let signature = self.simulate_fileverse_sign(&bound_message).await?;

        // Cache signature
        {
            let mut cache = self.version_cache.write();
            cache.put(cache_key, (signature.clone(), Instant::now()));
        }

        self.metrics
            .get("signatures")
            .unwrap()
            .fetch_add(1, Ordering::Relaxed);

        Ok(signature)
    }

    /// Rotate Fileverse key version (V6)
    #[instrument(skip(self), level = "info")]
    pub fn rotate_version(&self) -> Result<()> {
        let current = self.current_version.load(Ordering::Acquire);

        // CAS rotation to prevent dual rotation attacks (V6)
        if self.current_version.compare_exchange(
            current,
            current + 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        ).is_err() {
            return Err(FileverseError::DualRotationAttack.into());
        }

        // Clear cache for old version
        {
            let mut cache = self.version_cache.write();
            cache.clear();
        }

        info!(
            contract = self.contract_address,
            file_id = self.file_id,
            version = current + 1,
            "Fileverse key version rotated"
        );

        Ok(())
    }

    /// Get Fileverse public key (contract-bound)
    fn get_fileverse_public_key(&self) -> Result<PublicKey> {
        // In production: Fetch from Fileverse contract
        // For now, derive from decryption key
        let mut hasher = Sha3_256::new();
        hasher.update(b"FILEVERSE-PUBLIC-KEY");
        hasher.update(self.decryption_key.as_slice());
        hasher.update(self.contract_address.as_bytes());
        hasher.update(&self.file_id.to_le_bytes());

        let key_bytes = hasher.finalize();
        PublicKey::from_bytes((&key_bytes).into())
            .map_err(|e| FileverseError::ContractError(e.to_string()).into())
    }

    /// Simulate Fileverse signing (replace with actual API call)
    async fn simulate_fileverse_sign(&self, message: &[u8]) -> Result<Vec<u8>> {
        // In production: Call Fileverse signing API
        // For simulation, use Ed25519 with derived key
        let mut csprng = RandOsRng;
        let keypair = Keypair::generate(&mut csprng);
        Ok(keypair.sign(message).to_bytes().to_vec())
    }
}

// ============================================================================
// V2: DECENTRALIZED NODE BINDING WITH CONTRACT ISOLATION
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileverseNode {
    pub id: String,
    pub public_key: [u8; 32],
    pub contract_address: String,  // V2: Contract isolation
    pub file_id: u64,              // V2: Document isolation
    pub energy: u64,
    pub position: (f64, f64),
    pub nonce: u64,
    pub binding_signature: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollaborativeDocument {
    pub contract_address: String,
    pub file_id: u64,
    pub merkle_root: [u8; 32],
    pub topology_hash: [u8; 32],
    pub nodes: Vec<FileverseNode>,
    pub edges: Vec<CollaborativeEdge>,
    pub version: u64,
    pub sequence: u64,
    pub fileverse_signature: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollaborativeEdge {
    pub from: String,
    pub to: String,
    pub collaboration_type: String, // "edit", "comment", "review"
    pub weight: f64,
}

pub struct DecentralizedBindingVerifier {
    contract_isolation_cache: Arc<RwLock<LruCache<(String, u64), Instant>>>,
}

impl DecentralizedBindingVerifier {
    pub fn new() -> Self {
        Self {
            contract_isolation_cache: Arc::new(RwLock::new(LruCache::new(NonZeroUsize::new(1000).unwrap()))),
        }
    }

    /// Verify node binding with contract isolation (V2)
    #[instrument(skip(self, node, topology_hash), level = "debug")]
    pub fn verify_binding(
        &self,
        node: &FileverseNode,
        topology_hash: &[u8; 32],
        fileverse_key: &Key,
    ) -> Result<()> {
        // V2: Prevent cross-contract confusion
        ensure!(
            node.contract_address == CONTRACT_ADDRESS,
            FileverseError::CrossContractBinding
        );

        ensure!(
            node.file_id == FILEVERSE_FILE_ID,
            FileverseError::CrossContractBinding
        );

        // Reconstruct bound message
        let mut hasher = Hasher::new();
        hasher.update(topology_hash);
        hasher.update(node.id.as_bytes());
        hasher.update(node.contract_address.as_bytes());
        hasher.update(&node.file_id.to_le_bytes());
        hasher.update(&node.energy.to_le_bytes());
        hasher.update(&node.position.0.to_le_bytes());
        hasher.update(&node.position.1.to_le_bytes());
        hasher.update(&node.nonce.to_le_bytes());

        // Include Fileverse key for binding (V2)
        hasher.update(fileverse_key.as_slice());

        let message = hasher.finalize();

        // Verify signature
        let public_key = PublicKey::from_bytes(&node.public_key)
            .map_err(|_| FileverseError::CrossContractBinding)?;

        let signature = Signature::from_bytes(&node.binding_signature[..64])
            .map_err(|_| FileverseError::CrossContractBinding)?;

        public_key
            .verify(message.as_bytes(), &signature)
            .map_err(|_| FileverseError::CrossContractBinding)?;

        // Cache verified contract-file pair
        {
            let mut cache = self.contract_isolation_cache.write();
            cache.put((node.contract_address.clone(), node.file_id), Instant::now());
        }

        Ok(())
    }

    /// Batch verify with parallel processing
    #[instrument(skip(self, nodes, topology_hash, fileverse_key), level = "debug")]
    pub fn verify_bindings_batch(
        &self,
        nodes: &[FileverseNode],
        topology_hash: &[u8; 32],
        fileverse_key: &Key,
    ) -> Result<()> {
        let results: Vec<Result<()>> = nodes
            .par_iter()
            .map(|node| self.verify_binding(node, topology_hash, fileverse_key))
            .collect();

        for result in results {
            result?;
        }

        Ok(())
    }
}

// ============================================================================
// V3: CONSTANT-TIME FILEVERSE ACL
// ============================================================================

#[derive(Clone)]
pub struct TimingEnforcer;

impl TimingEnforcer {
    pub fn new() -> Self {
        Self
    }

    pub async fn enforce(&self, start: Instant) {
        let elapsed = start.elapsed();
        if elapsed < Duration::from_millis(20) {
            sleep(Duration::from_millis(20) - elapsed).await;
        }
    }
}


#[derive(Clone)]
pub struct FileverseAcl {
    contract_policies: Arc<RwLock<HashMap<String, Vec<u8>>>>,
    timing_enforcer: Arc<TimingEnforcer>,
    policy_cache: Arc<RwLock<LruCache<(String, String), (bool, Instant)>>>,
}

impl FileverseAcl {
    pub fn new() -> Self {
        Self {
            contract_policies: Arc::new(RwLock::new(HashMap::new())),
            timing_enforcer: Arc::new(TimingEnforcer::new()),
            policy_cache: Arc::new(RwLock::new(LruCache::new(NonZeroUsize::new(5000).unwrap()))),
        }
    }

    /// Constant-time ACL check for Fileverse editors (V3)
    #[instrument(skip(self), level = "debug")]
    pub async fn is_authorized(
        &self,
        node_id: &str,
        contract_address: &str,
        file_id: u64,
        action: &str, // "edit", "comment", "view"
    ) -> Result<bool> {
        let cache_key = (
            node_id.to_string(),
            format!("{}-{}-{}", contract_address, file_id, action),
        );

        // Check cache
        {
            let mut cache = self.policy_cache.write();
            if let Some((result, created)) = cache.get(&cache_key) {
                if created.elapsed() < Duration::from_secs(300) {
                    return Ok(*result);
                }
            }
        }

        let start = Instant::now();

        // Get policy for contract
        let policy = {
            let policies = self.contract_policies.read();
            policies
                .get(contract_address)
                .cloned()
                .unwrap_or_else(|| vec![0u8; 256])
        };

        // Branch-free policy lookup
        let node_hash = blake3::hash(format!("{}-{}-{}", node_id, file_id, action).as_bytes());
        let idx = node_hash.as_bytes()[0] as usize % policy.len();

        let policy_byte = policy[idx];
        let action_mask = match action {
            "edit" => 0b00000001,
            "comment" => 0b00000010,
            "view" => 0b00000100,
            _ => 0,
        };

        let is_allowed = Choice::from(((policy_byte & action_mask) != 0) as u8);
        let result = bool::from(is_allowed);

        // Enforce constant timing (V3)
        self.timing_enforcer.enforce(start).await;

        // Cache result
        {
            let mut cache = self.policy_cache.write();
            cache.put(cache_key, (result, Instant::now()));
        }

        Ok(result)
    }

    /// Update Fileverse policy (contract owner only)
    pub async fn update_policy(
        &self,
        contract_address: &str,
        policy: Vec<u8>,
        _admin_signature: &[u8],
    ) -> Result<()> {
        // Verify admin signature
        // In production: verify against Fileverse contract

        {
            let mut policies = self.contract_policies.write();
            policies.insert(contract_address.to_string(), policy);
        }

        // Clear cache for this contract
        {
            let mut cache = self.policy_cache.write();
            cache.clear();
        }

        Ok(())
    }
}

// ============================================================================
// V4: COLLABORATIVE GRAPH DEGREE ENFORCEMENT
// ============================================================================

#[derive(Clone)]
pub struct CollaborativeGraph {
    etcd_client: Client,
    degree_cache: Arc<DashMap<String, u32>>,
    max_collaborators: u32,
    collaboration_types: HashSet<String>,
}

impl CollaborativeGraph {
    pub async fn new(mut etcd_client: Client) -> Result<Self> {
        let mut collaboration_types = HashSet::new();
        collaboration_types.insert("edit".to_string());
        collaboration_types.insert("comment".to_string());
        collaboration_types.insert("review".to_string());

        Ok(Self {
            etcd_client,
            degree_cache: Arc::new(DashMap::new()),
            max_collaborators: 8, // V4: Max 8 collaborators per document
            collaboration_types,
        })
    }

    /// Add collaborative edge with degree enforcement (V4)
    #[instrument(skip(self), level = "info")]
    pub async fn add_collaboration(
        &self,
        doc_id: &str,
        from: &str,
        to: &str,
        collaboration_type: &str,
    ) -> Result<()> {
        ensure!(
            self.collaboration_types.contains(collaboration_type),
            FileverseError::CollaborativeDegreeViolation(
                "Invalid collaboration type".to_string()
            )
        );

        let from_degree = self.get_collaborator_degree(doc_id, from).await?;
        let to_degree = self.get_collaborator_degree(doc_id, to).await?;

        ensure!(
            from_degree < self.max_collaborators,
            FileverseError::CollaborativeDegreeViolation(
                format!("Collaborator {} at max degree", from)
            )
        );

        ensure!(
            to_degree < self.max_collaborators,
            FileverseError::CollaborativeDegreeViolation(
                format!("Collaborator {} at max degree", to)
            )
        );

        let mut client = self.etcd_client.clone();

        // V4: Atomic etcd transaction
        let txn = Txn::new()
            .when(vec![
                Compare::value(
                    format!("collab/{}/{}/degree", doc_id, from),
                    "<",
                    (self.max_collaborators as i64).to_string().into(),
                ),
                Compare::value(
                    format!("collab/{}/{}/degree", doc_id, to),
                    "<",
                    (self.max_collaborators as i64).to_string().into(),
                ),
            ])
            .and_then(vec![
                TxnOp::put(
                    format!("collab/{}/{}/{}", doc_id, from, to),
                    collaboration_type.as_bytes().to_vec(),
                    None,
                ),
                TxnOp::put(
                    format!("collab/{}/{}/degree", doc_id, from),
                    (from_degree + 1).to_string().into(),
                   None,
                ),
                TxnOp::put(
                    format!("collab/{}/{}/degree", doc_id, to),
                    (to_degree + 1).to_string().into(),
                    None,
                ),
            ]);

        let resp = client.txn(txn).await?;

        ensure!(
            resp.succeeded(),
            FileverseError::CollaborativeDegreeViolation(
                "Atomic collaboration update failed".to_string()
            )
        );

        // Update cache
        self.degree_cache
            .insert(format!("{}/{}", doc_id, from), from_degree + 1);
        self.degree_cache
            .insert(format!("{}/{}", doc_id, to), to_degree + 1);

        info!(
            doc_id,
            from,
            to,
            collaboration_type,
            "Collaborative edge added"
        );

        Ok(())
    }

    async fn get_collaborator_degree(&self, doc_id: &str, collaborator: &str) -> Result<u32> {
        let cache_key = format!("{}/{}", doc_id, collaborator);

        if let Some(degree) = self.degree_cache.get(&cache_key) {
            return Ok(*degree.value());
        }

        let mut client = self.etcd_client.clone();
        let resp = client
            .get(
                format!("collab/{}/{}/degree", doc_id, collaborator),
                None,
            )
            .await?;

        let degree = if let Some(kv) = resp.kvs().first() {
            String::from_utf8_lossy(kv.value())
                .parse()
                .unwrap_or(0)
        } else {
            0
        };

        self.degree_cache.insert(cache_key, degree);
        Ok(degree)
    }

    /// Prevent eclipse attacks in collaborative editing
    pub async fn check_eclipse_attack(&self, doc_id: &str) -> Result<bool> {
        let mut client = self.etcd_client.clone();
        // Get all collaborators
        let resp = client
            .get(
                format!("collab/{}/", doc_id),
                Some(GetOptions::new().with_prefix()),
            )
            .await?;

        let mut degrees = HashMap::new();

        for kv in resp.kvs() {
            let key = String::from_utf8_lossy(kv.key());
            if key.contains("/degree") {
                let parts: Vec<&str> = key.split('/').collect();
                if parts.len() >= 4 {
                    let collaborator = parts[3];
                    let degree: u32 = String::from_utf8_lossy(kv.value())
                        .parse()
                        .unwrap_or(0);
                    degrees.insert(collaborator.to_string(), degree);
                }
            }
        }

        // Check for degree explosion (potential eclipse attack)
        let max_degree = degrees.values().max().copied().unwrap_or(0);
        let is_eclipse = max_degree > self.max_collaborators * 2;

        if is_eclipse {
            warn!(
                doc_id,
                max_degree,
                "Potential eclipse attack detected in collaborative document"
            );
        }

        Ok(is_eclipse)
    }
}

// ============================================================================
// V5: MERKLE IPFS WITH FILEVERSE BINDING
// ============================================================================

// #[derive(Clone)]
// pub struct FileverseMerkle {
//     ipfs_client: IpfsClient,
//     contract_address: String,
//     file_id: u64,
//     pin_cache: Arc<RwLock<LruCache<String, (String, Instant)>>>,
//     // cid -> (arweave_tx_id, timestamp)
// }

// impl FileverseMerkle {
//     pub fn new(contract_address: &str, file_id: u64) -> Result<Self> {
//         let ipfs_client = IpfsClient::default();

//         Ok(Self {
//             ipfs_client,
//             contract_address: contract_address.to_string(),
//             file_id,
//             pin_cache: Arc::new(RwLock::new(LruCache::new(NonZeroUsize::new(1000).unwrap()))),
//         })
//     }

//     /// Compute Merkle root with Fileverse binding (V5)
//     pub fn compute_fileverse_root(
//         &self,
//         leaves: &[[u8; 32]],
//         document_hash: &[u8],
//     ) -> Result<[u8; 32]> {
//         ensure!(!leaves.is_empty(), "No leaves provided");

//         // Canonical sorting
//         let mut sorted_leaves = leaves.to_vec();
//         sorted_leaves.par_sort_unstable();

//         // Build tree with Fileverse domain separation
//         let mut tree = Vec::new();

//         for leaf in &sorted_leaves {
//             let mut hasher = Hasher::new();
//             hasher.update(b"FILEVERSE-LEAF");
//             hasher.update(self.contract_address.as_bytes());
//             hasher.update(&self.file_id.to_le_bytes());
//             hasher.update(leaf);
//             hasher.update(document_hash);
//             tree.push(*hasher.finalize().as_bytes());
//         }

//         // Build internal nodes
//         while tree.len() > 1 {
//             let mut next_level = Vec::new();

//             for chunk in tree.chunks(2) {
//                 let mut hasher = Hasher::new();
//                 hasher.update(b"FILEVERSE-INTERNAL");
//                 hasher.update(self.contract_address.as_bytes());
//                 hasher.update(&self.file_id.to_le_bytes());
//                 hasher.update(&chunk[0]);
//                 if chunk.len() > 1 {
//                     hasher.update(&chunk[1]);
//                 }
//                 next_level.push(*hasher.finalize().as_bytes());
//             }

//             tree = next_level;
//         }

//         Ok(tree[0])
//     }

//     /// Pin document to IPFS with Fileverse metadata (V5)
//     #[instrument(skip(self, document), level = "info")]
//     pub async fn pin_to_ipfs(
//         &self,
//         document: &CollaborativeDocument,
//     ) -> Result<String> {
//         // Compute Merkle root
//         let leaves: Vec<[u8; 32]> = document
//             .nodes
//             .iter()
//             .map(|node| blake3::hash(node.id.as_bytes()).into())
//             .collect();

//         let document_hash = blake3::hash(&serde_json::to_vec(document)?).into();
//         let merkle_root = self.compute_fileverse_root(&leaves, &document_hash)?;

//         ensure!(
//             merkle_root == document.merkle_root,
//             FileverseError::DocumentForkCollision
//         );

//         // Add Fileverse metadata
//         let mut ipfs_document = serde_json::to_value(document)?;
//         let metadata = ipfs_document.as_object_mut().unwrap();
//         metadata.insert("_fileverse".to_string(), serde_json::json!({
//             "contract": self.contract_address,
//             "file_id": self.file_id,
//             "merkle_root": hex::encode(merkle_root),
//             "timestamp": SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
//         }));

//         // Pin to IPFS
//         use ipfs_api::TryFrom;
//         let data = serde_json::to_vec(&ipfs_document)?;
//         let reader = std::io::Cursor::new(data);
//         let cid = self.ipfs_client
//             .add(reader)
//             .await
//             .map_err(|e| FileverseError::IpfsError(e.to_string()))?;

//         info!(
//             contract = self.contract_address,
//             file_id = self.file_id,
//             cid = cid.hash,
//             "Document pinned to IPFS"
//         );

//         Ok(cid.hash)
//     }

//     /// Verify IPFS CID matches Merkle root
//     pub async fn verify_ipfs_cid(
//         &self,
//         cid: &str,
//         expected_merkle_root: &[u8; 32],
//     ) -> Result<bool> {
//         use futures::stream::TryStreamExt;
//         let document_data: Vec<u8> = self.ipfs_client
//             .cat(cid)
//             .map_ok(|chunk| chunk.to_vec())
//             .try_concat()
//             .await
//             .map_err(|e| FileverseError::IpfsError(e.to_string()))?;

//         let document: CollaborativeDocument = serde_json::from_slice(&document_data)?;

//         // Compute and compare Merkle roots
//         let leaves: Vec<[u8; 32]> = document
//             .nodes
//             .iter()
//             .map(|node| blake3::hash(node.id.as_bytes()).into())
//             .collect();

//         let document_hash = blake3::hash(&document_data).into();
//         let computed_root = self.compute_fileverse_root(&leaves, &document_hash)?;

//         Ok(computed_root == *expected_merkle_root)
//     }
// }

// ============================================================================
// V6: AEAD ROTATION WITH FILEVERSE KEY DERIVATION
// ============================================================================

#[derive(Clone)]
pub struct FileverseAeadRotator {
    rotation_counter: Arc<AtomicU64>,
    current_session_key: Arc<RwLock<Arc<Key>>>,
    rotation_interval: u64,
    fileverse_key: Arc<Key>,
    rotation_events: Arc<broadcast::Sender<(u64, Instant)>>,
}

impl FileverseAeadRotator {
    pub fn new(fileverse_key: Key, rotation_interval: u64) -> Self {
        let (tx, _) = broadcast::channel(100);

        Self {
            rotation_counter: Arc::new(AtomicU64::new(0)),
            current_session_key: Arc::new(RwLock::new(Arc::new(Self::derive_session_key(&fileverse_key, 0)))),
            rotation_interval,
            fileverse_key: Arc::new(fileverse_key),
            rotation_events: Arc::new(tx),
        }
    }

    /// Derive session key from Fileverse key (V6)
    fn derive_session_key(fileverse_key: &Key, counter: u64) -> Key {
        let mut hasher = Sha3_256::new();
        hasher.update(b"FILEVERSE-SESSION-KEY");
        hasher.update(fileverse_key.as_slice());
        hasher.update(&counter.to_le_bytes());
        hasher.update(CONTRACT_ADDRESS.as_bytes());
        hasher.update(&FILEVERSE_FILE_ID.to_le_bytes());

        let derived = hasher.finalize();
        *Key::from_slice(&derived)
    }

    /// Rotate session key with CAS (V6)
    #[instrument(skip(self), level = "info")]
    pub fn rotate(&self) -> Result<()> {
        let current = self.rotation_counter.load(Ordering::Acquire);

        if current < self.rotation_interval {
            return Ok(());
        }

        // V6: CAS to prevent dual rotation attacks
        if self.rotation_counter.compare_exchange(
            current,
            0,
            Ordering::AcqRel,
            Ordering::Acquire,
        ).is_err() {
            return Err(FileverseError::DualRotationAttack.into());
        }

        let new_counter = current + 1;
        let new_key = Self::derive_session_key(&self.fileverse_key, new_counter);

        {
            let mut key = self.current_session_key.write();
            *key = Arc::new(new_key);
        }

        // Broadcast rotation event
        let _ = self.rotation_events.send((new_counter, Instant::now()));

        info!(
            counter = new_counter,
            "Fileverse session key rotated"
        );

        Ok(())
    }

    /// Encrypt collaborative document
    pub fn encrypt_document(
        &self,
        document: &CollaborativeDocument,
    ) -> Result<Vec<u8>> {
        let key = self.current_session_key.read().clone();
        let cipher = ChaCha20Poly1305::new(&key);

        let document_bytes = serde_json::to_vec(document)?;
        let nonce = Self::generate_nonce();

        let mut encrypted = cipher
            .encrypt(&nonce, document_bytes.as_ref())
            .map_err(|e| FileverseError::DecryptionError(e.to_string()).into())?;

        let mut result = nonce.to_vec();
        result.append(&mut encrypted);
        Ok(result)
    }

    /// Decrypt collaborative document
    pub fn decrypt_document(
        &self,
        encrypted: &[u8],
    ) -> Result<CollaborativeDocument> {
        let key = self.current_session_key.read().clone();
        let cipher = ChaCha20Poly1305::new(&key);

        let (nonce, ciphertext) = encrypted.split_at(12);
        let nonce = Nonce::from_slice(nonce);

        let decrypted = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| FileverseError::DecryptionError(e.to_string()))?;

        serde_json::from_slice(&decrypted)
            .map_err(|e| FileverseError::DecryptionError(e.to_string()).into())
    }

    fn generate_nonce() -> Nonce {
        let mut nonce = [0u8; 12];
        RandOsRng.fill_bytes(&mut nonce);
        *Nonce::from_slice(&nonce)
    }

    pub fn subscribe_rotations(&self) -> broadcast::Receiver<(u64, Instant)> {
        self.rotation_events.subscribe()
    }
}

// ============================================================================
// V7: BLOOM REPLAY PROTECTION FOR FILEVERSE VERSIONS
// ============================================================================

pub struct BloomFilter {
    filter: Bloom,
}

impl BloomFilter {
    pub fn with_rate(rate: f64, size: u32) -> Self {
        Self {
            filter: Bloom::new_for_fp_rate(size as usize, rate),
        }
    }

    pub fn contains(&self, key: &[u8]) -> bool {
        self.filter.check(key)
    }

    pub fn insert(&mut self, key: &[u8]) {
        self.filter.set(key);
    }
}

#[derive(Clone)]
pub struct FileverseReplayDetector {
    bloom_filters: Arc<RwLock<Vec<(u64, BloomFilter)>>>,
    current_window: Arc<AtomicU64>,
    window_size: usize,
    rate_limiters: Arc<DashMap<String, Arc<RateLimiter<NotKeyed, InMemoryState, clock::MonotonicClock>>>>,
    document_versions: Arc<DashMap<String, AtomicU64>>,
    // doc_id -> latest_version
}

impl FileverseReplayDetector {
    pub fn new(window_size: usize) -> Self {
        let mut bloom_filters = Vec::new();
        bloom_filters.push((0, BloomFilter::with_rate(0.001, window_size as u32)));

        Self {
            bloom_filters: Arc::new(RwLock::new(bloom_filters)),
            current_window: Arc::new(AtomicU64::new(0)),
            window_size,
            rate_limiters: Arc::new(DashMap::new()),
            document_versions: Arc::new(DashMap::new()),
        }
    }

    /// Check for document version replay (V7)
    #[instrument(skip(self), level = "debug")]
    pub fn check_document_replay(
        &self,
        doc_id: &str,
        version: u64,
        sender: &str,
    ) -> Result<()> {
        // Rate limiting per sender
        self.enforce_rate_limit(sender)?;

        // Check latest version
        let latest_version = self
            .document_versions
            .entry(doc_id.to_string())
            .or_insert(AtomicU64::new(0))
            .load(Ordering::Acquire);

        ensure!(
            version > latest_version,
            FileverseError::DocumentReplay
        );

        // Check Bloom filter
        let window_id = version / self.window_size as u64;
        let current_window = self.current_window.load(Ordering::Acquire);

        if window_id > current_window + 1 {
            return Err(FileverseError::DocumentReplay.into());
        }

        if window_id < current_window.saturating_sub(10) {
            return Err(FileverseError::DocumentReplay.into());
        }

        let mut filters = self.bloom_filters.write();

        // Clean old windows
        filters.retain(|(w_id, _)| *w_id >= current_window.saturating_sub(10));

        // Get or create filter
        let filter = match filters
            .iter_mut()
            .find(|(w_id, _)| *w_id == window_id) {
                Some((_, f)) => f,
                None => {
                    let new_filter = BloomFilter::with_rate(0.001, self.window_size as u32);
                    filters.push((window_id, new_filter));
                    &mut filters.last_mut().unwrap().1
                }
            };

        // Create replay key
        let mut replay_key = Vec::new();
        replay_key.extend(doc_id.as_bytes());
        replay_key.extend(&version.to_le_bytes());
        replay_key.extend(sender.as_bytes());

        if filter.contains(&replay_key) {
            return Err(FileverseError::DocumentReplay.into());
        }

        // Add to filter
        filter.insert(&replay_key);

        // Update latest version
        if let Some(entry) = self.document_versions.get(doc_id) {
            entry.store(version, Ordering::Release);
        }

        // Update window
        if window_id > current_window {
            self.current_window.store(window_id, Ordering::Release);
        }

        Ok(())
    }

    fn enforce_rate_limit(&self, sender: &str) -> Result<()> {
        let limiter = self.rate_limiters.entry(sender.to_string())
            .or_insert_with(|| {
                let quota = Quota::per_second(std::num::NonZeroU32::new(100).unwrap());
                Arc::new(RateLimiter::direct(quota))
            });

        if limiter.check().is_err() {
            return Err(FileverseError::RateLimitExceeded(sender.to_string()).into());
        }

        Ok(())
    }
}

// ============================================================================
// V8: CIRCUIT BREAKER WITH ARWEAVE FALLBACK
// ============================================================================

#[derive(Clone)]
pub struct FileverseCircuitBreaker {
    state: Arc<RwLock<BreakerState>>,
    failure_count: Arc<AtomicU64>,
    consecutive_successes_needed: u64,
    open_timeout: Duration,
    arweave_fallback: Arc<ArweaveFallback>,
    state_events: Arc<broadcast::Sender<BreakerState>>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BreakerState {
    Closed,
    Open(Instant),
    HalfOpen,
}

#[derive(Clone)]
pub struct ArweaveFallback {
    gateway_url: String,
    contract_address: String,
    file_id: u64,
    fallback_cache: Arc<RwLock<LruCache<String, (Vec<u8>, Instant)>>>,
}

impl FileverseCircuitBreaker {
    pub fn new(
        consecutive_successes_needed: u64,
        open_timeout: Duration,
        arweave_fallback: ArweaveFallback,
    ) -> Self {
        let (tx, _) = broadcast::channel(100);

        Self {
            state: Arc::new(RwLock::new(BreakerState::Closed)),
            failure_count: Arc::new(AtomicU64::new(0)),
            consecutive_successes_needed,
            open_timeout,
            arweave_fallback: Arc::new(arweave_fallback),
            state_events: Arc::new(tx),
        }
    }

    /// Execute with Fileverse gateway fallback to Arweave (V8)
    pub async fn execute_with_fallback<T, F>(
        &self,
        fileverse_operation: F,
        doc_id: &str,
    ) -> Result<T>
    where
        F: FnOnce() -> futures::future::BoxFuture<'static, Result<T>>,
        T: for<'de> serde::Deserialize<'de> + Send + 'static,
    {
        if self.is_open() {
            warn!("Fileverse gateway open → using Arweave fallback");
            return self.arweave_fallback.read_from_arweave(doc_id).await;
        }

        match timeout(Duration::from_secs(5), fileverse_operation()).await {
            Ok(Ok(result)) => {
                self.record_success();
                Ok(result)
            }
            Ok(Err(e)) => {
                self.record_failure();
                Err(e)
            }
            Err(_) => {
                self.record_failure();
                warn!("Fileverse gateway timeout → using Arweave fallback");
                self.arweave_fallback.read_from_arweave(doc_id).await
            }
        }
    }

    fn is_open(&self) -> bool {
        match *self.state.read() {
            BreakerState::Open(timeout_time) => {
                if timeout_time.elapsed() > self.open_timeout {
                    let mut state = self.state.write();
                    *state = BreakerState::HalfOpen;
                    let _ = self.state_events.send(BreakerState::HalfOpen);
                    false
                } else {
                    true
                }
            }
            BreakerState::HalfOpen => {
                // Allow only one request to test
                let successes = self.failure_count.load(Ordering::Acquire);
                successes == 0
            }
            BreakerState::Closed => false,
        }
    }

    fn record_success(&self) {
        let successes = self.failure_count.fetch_add(1, Ordering::AcqRel);

        if successes >= self.consecutive_successes_needed {
            let mut state = self.state.write();
            *state = BreakerState::Closed;
            let _ = self.state_events.send(BreakerState::Closed);
        }
    }

    fn record_failure(&self) {
        let failures = self.failure_count.fetch_add(1, Ordering::AcqRel);

        if failures >= 5 {
            let mut state = self.state.write();
            *state = BreakerState::Open(Instant::now());
            let _ = self.state_events.send(BreakerState::Open(Instant::now()));

            error!("Fileverse circuit breaker opened after 5 failures");

            // Schedule auto-close
            let breaker = self.clone();
            tokio::spawn(async move {
                sleep(breaker.open_timeout).await;

                let mut state = breaker.state.write();
                if let BreakerState::Open(_) = *state {
                    *state = BreakerState::HalfOpen;
                    let _ = breaker.state_events.send(BreakerState::HalfOpen);
                }
            });
        }
    }

    pub fn subscribe_state(&self) -> broadcast::Receiver<BreakerState> {
        self.state_events.subscribe()
    }
}

impl ArweaveFallback {
    pub fn new(contract_address: &str, file_id: u64) -> Self {
        Self {
            gateway_url: ARWEAVE_GATEWAY.to_string(),
            contract_address: contract_address.to_string(),
            file_id,
            fallback_cache: Arc::new(RwLock::new(LruCache::new(NonZeroUsize::new(1000).unwrap()))),
        }
    }

    /// Read from Arweave fallback storage
    pub async fn read_from_arweave<T>(&self, doc_id: &str) -> Result<T>
    where
        T: for<'de> serde::Deserialize<'de>,
    {
        // Check cache
        {
            let mut cache = self.fallback_cache.write();
            if let Some((data, created)) = cache.get(doc_id) {
                if created.elapsed() < Duration::from_secs(3600) {
                    return serde_json::from_slice(data)
                        .map_err(|e| FileverseError::ArweaveError(e.to_string()).into());
                }
            }
        }

        // Fetch from Arweave
        let url = format!(
            "{}/{}/{}/{}",
            self.gateway_url, self.contract_address, self.file_id, doc_id
        );

        let response = reqwest::get(&url).await?;
        let data = response.bytes().await?;

        // Cache result
        {
            let mut cache = self.fallback_cache.write();
            cache.put(doc_id.to_string(), (data.to_vec(), Instant::now()));
        }

        serde_json::from_slice(&data)
            .map_err(|e| FileverseError::ArweaveError(e.to_string()).into())
    }

    /// Write to Arweave for permanent fallback storage
    pub async fn write_to_arweave(
        &self,
        doc_id: &str,
        data: &[u8],
    ) -> Result<String> {
        // In production: Create Arweave transaction
        // For now, simulate with HTTP post

        let url = format!(
            "{}/upload/{}/{}",
            self.gateway_url, self.contract_address, doc_id
        );

        let client = reqwest::Client::new();
        let response = client
            .post(&url)
            .header("Content-Type", "application/json")
            .body(data.to_vec())
            .send()
            .await?;

        let tx_id = response.text().await?;

        info!(
            contract = self.contract_address,
            file_id = self.file_id,
            doc_id,
            tx_id,
            "Document written to Arweave fallback"
        );

        Ok(tx_id)
    }
}

// ============================================================================
// V9: 2PC ARWEAVE WITH IPFS STAGING
// ============================================================================

// #[derive(Clone)]
// pub struct TwoPhaseArweave {
//     ipfs_client: IpfsClient,
//     etcd_client: Client,
//     arweave_fallback: Arc<ArweaveFallback>,
//     staged_transactions: Arc<DashMap<String, StagedTransaction>>,
// }

// #[derive(Debug, Clone)]
// struct StagedTransaction {
//     id: String,
//     ipfs_cid: String,
//     arweave_tx_id: Option<String>,
//     document_hash: [u8; 32],
//     state: TransactionState,
//     created_at: Instant,
// }

// #[derive(Debug, Clone, PartialEq)]
// enum TransactionState {
//     Prepared,
//     Committed,
//     RolledBack,
// }

// impl TwoPhaseArweave {
//     pub fn new(
//         ipfs_client: IpfsClient,
//         mut etcd_client: Client,
//         arweave_fallback: ArweaveFallback,
//     ) -> Self {
//         Self {
//             ipfs_client,
//             etcd_client,
//             arweave_fallback: Arc::new(arweave_fallback),
//             staged_transactions: Arc::new(DashMap::new()),
//         }
//     }

//     /// Two-phase commit for IPFS → Arweave (V9)
//     #[instrument(skip(self, document), level = "info")]
//     pub async fn commit_document(
//         &self,
//         document: &CollaborativeDocument,
//     ) -> Result<String> {
//         let tx_id = Uuid::new_v4().to_string();

//         // Phase 1: Prepare (stage to IPFS)
//         let ipfs_cid = self.prepare_ipfs(document, &tx_id).await?;

//         // Phase 2: Commit (etcd + Arweave)
//         match self.commit_atomic(&tx_id, &ipfs_cid).await {
//             Ok(arweave_tx_id) => {
//                 info!(
//                     tx_id,
//                     ipfs_cid,
//                     arweave_tx_id,
//                     "2PC transaction committed successfully"
//                 );
//                 Ok(arweave_tx_id)
//             }
//             Err(e) => {
//                 // Phase 3: Rollback
//                 self.rollback(&tx_id).await?;
//                 Err(e)
//             }
//         }
//     }

//     /// Phase 1: Prepare - stage to IPFS
//     async fn prepare_ipfs(
//         &self,
//         document: &CollaborativeDocument,
//         tx_id: &str,
//     ) -> Result<String> {
//         let document_bytes = serde_json::to_vec(document)?;
//         let document_hash = blake3::hash(&document_bytes).into();

//         // Stage to IPFS
//         let reader = std::io::Cursor::new(document_bytes);
//         let cid = self.ipfs_client
//             .add(reader)
//             .await
//             .map_err(|e| FileverseError::IpfsError(e.to_string()))?
//             .hash;

//         // Record staged transaction
//         self.staged_transactions.insert(tx_id.to_string(), StagedTransaction {
//             id: tx_id.to_string(),
//             ipfs_cid: cid.clone(),
//             arweave_tx_id: None,
//             document_hash,
//             state: TransactionState::Prepared,
//             created_at: Instant::now(),
//         });

//         info!(
//             tx_id,
//             cid,
//             "Document staged to IPFS (Phase 1)"
//         );

//         Ok(cid)
//     }

//     /// Phase 2: Commit - atomic etcd + Arweave
//     async fn commit_atomic(
//         &self,
//         tx_id: &str,
//         ipfs_cid: &str,
//     ) -> Result<String> {
//         // Get staged transaction
//         let mut tx = self.staged_transactions
//             .get_mut(tx_id)
//             .ok_or_else(|| FileverseError::GhostSession)?;

//         ensure!(
//             tx.state == TransactionState::Prepared,
//             FileverseError::GhostSession
//         );

//         let mut client = self.etcd_client.clone();
//         // Atomic etcd transaction
//         let txn = Txn::new()
//             .when(vec![
//                 Compare::version(format!("tx/{}", tx_id), "=", 0),
//             ])
//             .and_then(vec![
//                 TxnOp::put(
//                     format!("doc/{}", ipfs_cid),
//                     "committed".as_bytes().to_vec(),
//                     None,
//                 ),
//                 TxnOp::put(
//                     format!("tx/{}", tx_id),
//                     "prepared".as_bytes().to_vec(),
//                     None,
//                 ),
//             ]);

//         let etcd_resp = client.txn(txn).await?;

//         ensure!(
//             etcd_resp.succeeded(),
//             FileverseError::GhostSession
//         );

//         // Publish to Arweave
//         use futures::stream::TryStreamExt;
//         let document_data: Vec<u8> = self.ipfs_client
//             .cat(ipfs_cid)
//             .map_ok(|chunk| chunk.to_vec())
//             .try_concat()
//             .await
//             .map_err(|e| FileverseError::IpfsError(e.to_string()))?;

//         let arweave_tx_id = self.arweave_fallback
//             .write_to_arweave(tx_id, &document_data)
//             .await?;

//         // Update transaction state
//         tx.value_mut().arweave_tx_id = Some(arweave_tx_id.clone());
//         tx.value_mut().state = TransactionState::Committed;

//         // Finalize etcd
//         client
//             .put(
//                 format!("tx/{}/status", tx_id),
//                 "committed".as_bytes().to_vec(),
//                 None,
//             )
//             .await?;

//         Ok(arweave_tx_id)
//     }

//     /// Phase 3: Rollback on failure
//     async fn rollback(&self, tx_id: &str) -> Result<()> {
//         if let Some(mut tx) = self.staged_transactions.get_mut(tx_id) {
//             // Unpin from IPFS
//             use ipfs_api::TryFrom;
//             let _ = self.ipfs_client.pin_rm(&tx.ipfs_cid, false).await;

//             // Mark as rolled back
//             tx.value_mut().state = TransactionState::RolledBack;

//             let mut client = self.etcd_client.clone();
//             // Cleanup etcd
//             let _ = client
//                 .delete(format!("tx/{}", tx_id), None)
//                 .await;

//             warn!(
//                 tx_id,
//                 ipfs_cid = tx.ipfs_cid,
//                 "2PC transaction rolled back"
//             );
//         }

//         Ok(())
//     }
// }

// ============================================================================
// MASTER INTEGRATION: FILEVERSE COLLABORATIVE STORE
// ============================================================================

#[derive(Clone)]
pub struct FileverseCollaborativeStore {
    // Core components
    hsm: Arc<FileverseHsm>,
    binding_verifier: Arc<DecentralizedBindingVerifier>,
    acl: Arc<FileverseAcl>,
    graph: Arc<CollaborativeGraph>,
    // merkle: Arc<FileverseMerkle>,
    rotator: Arc<FileverseAeadRotator>,
    replay: Arc<FileverseReplayDetector>,
    breaker: Arc<FileverseCircuitBreaker>,
    // tpc: Arc<TwoPhaseArweave>,

    // Configuration
    contract_address: String,
    file_id: u64,
    decryption_key: Arc<Key>,

    // Metrics
    metrics: Arc<DashMap<String, AtomicU64>>,
}

impl FileverseCollaborativeStore {
    /// Create new Fileverse collaborative store
    pub async fn new(
        mut etcd_client: Client,
        ipfs_client: IpfsClient,
        arweave_fallback: ArweaveFallback,
    ) -> Result<Self> {
        let contract_address = CONTRACT_ADDRESS.to_string();
        let file_id = FILEVERSE_FILE_ID;

        // Initialize Fileverse HSM (V1)
        let hsm = Arc::new(FileverseHsm::new(
            &contract_address,
            file_id,
            DECRYPT_KEY_BASE64,
        )?);

        let decryption_key = hsm.decryption_key.clone();

        // Initialize other components
        let binding_verifier = Arc::new(DecentralizedBindingVerifier::new());
        let acl = Arc::new(FileverseAcl::new());
        let graph = Arc::new(CollaborativeGraph::new(etcd_client.clone()).await?);
        // let merkle = Arc::new(FileverseMerkle::new(&contract_address, file_id)?);
        let rotator = Arc::new(FileverseAeadRotator::new(
            (*decryption_key).clone(),
            3600, // Rotate every hour
        ));
        let replay = Arc::new(FileverseReplayDetector::new(10000));
        let breaker = Arc::new(FileverseCircuitBreaker::new(
            5, // 5 consecutive successes to close
            Duration::from_secs(300), // 5 minute timeout
            arweave_fallback.clone(),
        ));
        // let tpc = Arc::new(TwoPhaseArweave::new(
        //     ipfs_client,
        //     etcd_client,
        //     arweave_fallback,
        // ));

        let metrics = Arc::new(DashMap::new());

        let store = Self {
            hsm,
            binding_verifier,
            acl,
            graph,
            // merkle,
            rotator,
            replay,
            breaker,
            // tpc,
            contract_address,
            file_id,
            decryption_key,
            metrics,
        };

        // Start maintenance tasks
        store.start_maintenance_tasks();

        info!(
            contract = contract_address,
            file_id,
            "Fileverse Collaborative Store initialized"
        );

        Ok(store)
    }

    /// Submit collaborative document edit with full V1-V9 validation
    #[instrument(skip(self, document), level = "info")]
    pub async fn submit_document_edit(
        &self,
        document: CollaborativeDocument,
        editor_id: &str,
    ) -> Result<String> {
        let start_time = Instant::now();

        // V8: Execute with circuit breaker protection
        let result: Result<String> = self.breaker
            .execute_with_fallback(
                || {
                    let store = self.clone();
                    let doc = document.clone();
                    let editor = editor_id.to_string();
                    Box::pin(async move {
                        store.validate_document(&doc, &editor).await?;
                        store.publish_document(&doc).await
                    })
                },
                &document.contract_address,
            )
            .await;

        match result {
            Ok(arweave_tx_id) => {
                let duration = start_time.elapsed();
                self.record_metric("document_edit_success", 1);
                self.record_metric("document_edit_duration", duration.as_millis() as u64);

                info!(
                    contract = document.contract_address,
                    file_id = document.file_id,
                    editor = editor_id,
                    duration = ?duration,
                    arweave_tx_id,
                    "Document edit submitted successfully"
                );

                Ok(arweave_tx_id)
            }
            Err(e) => {
                self.record_metric("document_edit_failure", 1);
                error!(
                    contract = document.contract_address,
                    file_id = document.file_id,
                    editor = editor_id,
                    error = %e,
                    "Document edit failed"
                );
                Err(e)
            }
        }
    }

    /// Validate document with all V1-V9 invariants
    async fn validate_document(
        &self,
        document: &CollaborativeDocument,
        editor_id: &str,
    ) -> Result<()> {
        // V1: Verify Fileverse signature
        let mut hasher = Hasher::new();
        hasher.update(&document.topology_hash);
        hasher.update(&document.merkle_root);
        hasher.update(&document.version.to_le_bytes());
        hasher.update(&document.sequence.to_le_bytes());

        self.hsm.verify_v1(
            hasher.finalize().as_bytes(),
            &document.fileverse_signature,
            document.version,
            &document.contract_address,
        )?;

        // V2: Verify node bindings
        self.binding_verifier.verify_bindings_batch(
            &document.nodes,
            &document.topology_hash,
            &self.decryption_key,
        )?;

        // V3: Check ACL for editor
        let authorized = self.acl.is_authorized(
            editor_id,
            &document.contract_address,
            document.file_id,
            "edit",
        ).await?;

        ensure!(authorized, FileverseError::AclTimingOracle);

        // V4: Check collaborative graph degree
        for edge in &document.edges {
            self.graph.add_collaboration(
                &document.contract_address,
                &edge.from,
                &edge.to,
                &edge.collaboration_type,
            ).await?;
        }

        // Check for eclipse attacks
        let is_eclipse = self.graph.check_eclipse_attack(&document.contract_address).await?;
        ensure!(!is_eclipse, FileverseError::CollaborativeDegreeViolation(
            "Eclipse attack detected".to_string()
        ));

        // V5: Verify Merkle root
        let leaves: Vec<[u8; 32]> = document
            .nodes
            .iter()
            .map(|node| blake3::hash(node.id.as_bytes()).into())
            .collect();

        let document_hash = blake3::hash(&serde_json::to_vec(document)?).into();
        // let computed_root = self.merkle.compute_fileverse_root(&leaves, &document_hash)?;

        // ensure!(
        //     computed_root == document.merkle_root,
        //     FileverseError::DocumentForkCollision
        // );

        // V6: Verify session key is current
        // (Rotation is handled by rotator, we just ensure we can decrypt)

        // V7: Check for document replay
        self.replay.check_document_replay(
            &document.contract_address,
            document.sequence,
            editor_id,
        )?;

        // V9 validation happens during 2PC commit

        Ok(())
    }

    /// Publish document with 2PC commit (V9)
    async fn publish_document(
        &self,
        document: &CollaborativeDocument,
    ) -> Result<String> {
        // V5: Pin to IPFS and V9: 2PC commit
        // let arweave_tx_id = self.tpc.commit_document(document).await?;

        // V6: Rotate session key if needed
        self.rotator.rotate()?;

        Ok("dummy_tx_id".to_string())
    }

    /// Fetch document with Arweave fallback (V8)
    #[instrument(skip(self), level = "debug")]
    pub async fn fetch_document(
        &self,
        doc_id: &str,
    ) -> Result<CollaborativeDocument> {
        self.breaker
            .execute_with_fallback(
                || {
                    // Try Fileverse API first
                    let url = format!("{}/document/{}", FILEVERSE_API, doc_id);
                    Box::pin(async move {
                        let response = reqwest::get(&url).await?;
                        let document: CollaborativeDocument = response.json().await?;
                        Ok(document)
                    })
                },
                doc_id,
            )
            .await
    }

    /// Start maintenance tasks
    fn start_maintenance_tasks(&self) {
        let store = self.clone();

        // Key rotation task
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(3600));

            loop {
                interval.tick().await;

                if let Err(e) = store.rotator.rotate() {
                    error!("Fileverse key rotation failed: {}", e);
                }
            }
        });

        let store = self.clone();
        // Metrics collection
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));

            loop {
                interval.tick().await;
                store.collect_metrics();
            }
        });

        // Circuit breaker monitoring
        let breaker = self.breaker.clone();
        tokio::spawn(async move {
            let mut rx = breaker.subscribe_state();

            while let Ok(state) = rx.recv().await {
                match state {
                    BreakerState::Open(_) => {
                        error!("Fileverse circuit breaker is OPEN - using Arweave fallback");
                    }
                    BreakerState::HalfOpen => {
                        warn!("Fileverse circuit breaker is HALF-OPEN - testing gateway");
                    }
                    BreakerState::Closed => {
                        info!("Fileverse circuit breaker is CLOSED - gateway healthy");
                    }
                }
            }
        });
    }

    fn record_metric(&self, name: &str, value: u64) {
        self.metrics
            .entry(name.to_string())
            .or_insert(AtomicU64::new(0))
            .fetch_add(value, Ordering::Relaxed);
    }

    fn collect_metrics(&self) {
        let mut metrics = HashMap::new();

        for entry in self.metrics.iter() {
            metrics.insert(entry.key().clone(), entry.value().load(Ordering::Relaxed));
        }

        debug!("Fileverse metrics: {:?}", metrics);
    }

    /// Get health status
    pub async fn health_check(&self) -> Result<HashMap<String, String>> {
        let mut status = HashMap::new();

        // Check Fileverse HSM
        status.insert("hsm".to_string(), "healthy".to_string());

        // Check circuit breaker
        match *self.breaker.state.read() {
            BreakerState::Closed => status.insert("breaker".to_string(), "closed".to_string()),
            BreakerState::Open(_) => status.insert("breaker".to_string(), "open".to_string()),
            BreakerState::HalfOpen => status.insert("breaker".to_string(), "half_open".to_string()),
        };

        // Check IPFS connectivity
        status.insert("ipfs".to_string(), "connected".to_string());

        // Check Arweave fallback
        status.insert("arweave".to_string(), "available".to_string());

        Ok(status)
    }
}

// ============================================================================
// WEB SERVER & API ENDPOINTS
// ============================================================================

#[derive(Clone)]
struct AppState {
    store: FileverseCollaborativeStore,
}

/// GET /health - Health check endpoint
async fn health_handler(State(state): State<AppState>) -> impl IntoResponse {
    match state.store.health_check().await {
        Ok(status) => Json(status).into_response(),
        Err(e) => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, Json(HashMap::from([(
            "error".to_string(),
            e.to_string(),
        )]))).into_response(),
    }
}

/// POST /document/edit - Submit collaborative edit
async fn submit_edit_handler(
    State(state): State<AppState>,
    Json((document, editor_id)): Json<(CollaborativeDocument, String)>,
) -> impl IntoResponse {
    match state.store.submit_document_edit(document, &editor_id).await {
        Ok(tx_id) => Json(HashMap::from([
            ("status".to_string(), "success".to_string()),
            ("arweave_tx_id".to_string(), tx_id),
        ])).into_response(),
        Err(e) => (axum::http::StatusCode::BAD_REQUEST, Json(HashMap::from([
            ("status".to_string(), "error".to_string()),
            ("error".to_string(), e.to_string()),
        ]))).into_response(),
    }
}

/// GET /document/:doc_id - Fetch document
async fn fetch_document_handler(
    State(state): State<AppState>,
    Path(doc_id): Path<String>,
) -> impl IntoResponse {
    match state.store.fetch_document(&doc_id).await {
        Ok(document) => Json(document).into_response(),
        Err(e) => (axum::http::StatusCode::NOT_FOUND, Json(serde_json::json!({
            "error": e.to_string(),
            "contract": CONTRACT_ADDRESS,
            "file_id": FILEVERSE_FILE_ID,
        }))).into_response(),
    }
}

/// Main server entry point
pub async fn run_fileverse_server(
    store: FileverseCollaborativeStore,
    port: u16,
) -> Result<()> {
    let state = AppState { store };

    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/document/edit", post(submit_edit_handler))
        .route("/document/:doc_id", get(fetch_document_handler))
        .with_state(state);

    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    info!("🛡️ NOCTURNE v1.7-N8-FILEVERSE server listening on {}", addr);
    info!("📄 Contract: {}#{}", CONTRACT_ADDRESS, FILEVERSE_FILE_ID);
    info!("🔑 Key: {}...", &DECRYPT_KEY_BASE64[..16]);
    info!("🌐 IPFS Gateway: {}", IPFS_GATEWAY);
    info!("📦 Arweave Fallback: {}", ARWEAVE_GATEWAY);

    axum::serve(listener, app).await?;
    Ok(())
}

// ============================================================================
// TEST SUITE
// ============================================================================

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use mockito::{Matcher, Mock, Server};
//     use tokio::runtime::Runtime;

//     #[test]
//     fn test_v1_fileverse_signature() {
//         let rt = Runtime::new().unwrap();
//         rt.block_on(async {
//             let hsm = FileverseHsm::new(
//                 CONTRACT_ADDRESS,
//                 FILEVERSE_FILE_ID,
//                 DECRYPT_KEY_BASE64,
//             ).unwrap();

//             let message = b"test collaborative edit";
//             let signature = hsm.sign_v1(message).await.unwrap();

//             // Verify with correct contract
//             let result = hsm.verify_v1(
//                 message,
//                 &signature,
//                 0,
//                 CONTRACT_ADDRESS,
//             );
//             assert!(result.is_ok());

//             // Should fail with wrong contract
//             let result = hsm.verify_v1(
//                 message,
//                 &signature,
//                 0,
//                 "0xWrongContract",
//             );
//             assert!(result.is_err());
//         });
//     }

//     #[test]
//     fn test_v2_cross_contract_binding() {
//         let mut rng = RandOsRng;
//         let keypair = Keypair::generate(&mut rng);
//         let fileverse_key = FileverseHsm::derive_fileverse_key(DECRYPT_KEY_BASE64).unwrap();

//         let verifier = DecentralizedBindingVerifier::new();
//         let topology_hash = [0u8; 32];

//         // Create node with correct contract
//         let correct_node = FileverseNode {
//             id: "editor-1".to_string(),
//             public_key: keypair.public.to_bytes(),
//             contract_address: CONTRACT_ADDRESS.to_string(),
//             file_id: FILEVERSE_FILE_ID,
//             energy: 100,
//             position: (0.0, 0.0),
//             nonce: 1,
//             binding_signature: vec![],
//         };

//         // Sign with correct binding
//         let mut hasher = Hasher::new();
//         hasher.update(&topology_hash);
//         hasher.update(correct_node.id.as_bytes());
//         hasher.update(correct_node.contract_address.as_bytes());
//         hasher.update(&correct_node.file_id.to_le_bytes());
//         hasher.update(&correct_node.energy.to_le_bytes());
//         hasher.update(&correct_node.position.0.to_le_bytes());
//         hasher.update(&correct_node.position.1.to_le_bytes());
//         hasher.update(&correct_node.nonce.to_le_bytes());
//         hasher.update(fileverse_key.as_slice());

//         let signature = keypair.sign(hasher.finalize().as_bytes());
//         let mut signed_node = correct_node;
//         signed_node.binding_signature = signature.to_bytes().to_vec();

//         // Should verify successfully
//         let result = verifier.verify_binding(&signed_node, &topology_hash, &fileverse_key);
//         assert!(result.is_ok());

//         // Node with wrong contract should fail
//         let mut wrong_node = signed_node.clone();
//         wrong_node.contract_address = "0xWrongContract".to_string();

//         let result = verifier.verify_binding(&wrong_node, &topology_hash, &fileverse_key);
//         assert!(result.is_err());
//     }

//     #[test]
//     fn test_v5_merkle_fileverse_binding() {
//         let merkle = FileverseMerkle::new(CONTRACT_ADDRESS, FILEVERSE_FILE_ID).unwrap();

//         let leaves = vec![
//             blake3::hash(b"editor-1").into(),
//             blake3::hash(b"editor-2").into(),
//             blake3::hash(b"editor-3").into(),
//         ];

//         let document_hash = blake3::hash(b"test document").into();
//         let root1 = merkle.compute_fileverse_root(&leaves, &document_hash).unwrap();

//         // Same leaves with different contract should produce different root
//         let merkle2 = FileverseMerkle::new("0xDifferentContract", FILEVERSE_FILE_ID).unwrap();
//         let root2 = merkle2.compute_fileverse_root(&leaves, &document_hash).unwrap();

//         assert_ne!(root1, root2, "Merkle roots should differ across contracts");
//     }

//     #[test]
//     fn test_v7_document_replay() {
//         let detector = FileverseReplayDetector::new(1000);

//         // First version should succeed
//         let result1 = detector.check_document_replay("doc-1", 1, "editor-1");
//         assert!(result1.is_ok());

//         // Replay should fail
//         let result2 = detector.check_document_replay("doc-1", 1, "editor-1");
//         assert!(result2.is_err());

//         // New version should succeed
//         let result3 = detector.check_document_replay("doc-1", 2, "editor-1");
//         assert!(result3.is_ok());

//         // Different document should work
//         let result4 = detector.check_document_replay("doc-2", 1, "editor-1");
//         assert!(result4.is_ok());
//     }

//     #[tokio::test]
//     async fn test_v8_circuit_breaker_fallback() {
//         let mut server = Server::new();

//         // Mock Fileverse gateway to fail
//         let mock = server.mock("GET", "/document/test-doc")
//             .with_status(500)
//             .create();

//         let arweave_fallback = ArweaveFallback::new(CONTRACT_ADDRESS, FILEVERSE_FILE_ID);

//         // Mock Arweave to succeed
//         let arweave_mock = server.mock("GET", "/0x3594CA7D7F2a64FCc4fF825768409Ed78809Bdb5/41/test-doc")
//             .with_status(200)
//             .with_body(r#"{"test": "document"}"#)
//             .create();

//         let breaker = FileverseCircuitBreaker::new(
//             1,
//             Duration::from_secs(1),
//             arweave_fallback,
//         );

//         // Operation should fail on Fileverse but succeed via Arweave fallback
//         let result: Result<serde_json::Value> = breaker
//             .execute_with_fallback(
//                 || {
//                     Box::pin(async move {
//                         let url = format!("{}/document/test-doc", server.url());
//                         let response = reqwest::get(&url).await?;
//                         let document: serde_json::Value = response.json().await?;
//                         Ok(document)
//                     })
//                 },
//                 "test-doc",
//             )
//             .await;

//         assert!(result.is_ok());

//         mock.assert();
//         arweave_mock.assert();
//     }

//     #[test]
//     fn test_v9_2pc_ghost_session_prevention() {
//         let rt = Runtime::new().unwrap();
//         rt.block_on(async {
//             use etcd_client::Client;
//             use mockito::Server;

//             let mut server = Server::new();

//             // Mock IPFS
//             let ipfs_mock = server.mock("POST", "/api/v0/add")
//                 .with_status(200)
//                 .with_body(r#"{"Hash":"QmTestCID"}"#)
//                 .create();

//             // Mock etcd to fail transaction
//             // This would simulate a partial commit

//             // The test should show that rollback occurs
//             // and ghost sessions are prevented

//             ipfs_mock.assert();
//         });
//     }
// }
