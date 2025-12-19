"""
Generates valid cryptographic data for all golden vector categories (canonical, bls, integration).
"""

import json
import os
import binascii
import hashlib
from typing import List
from blspy import AugSchemeMPL, PrivateKey
import canonicaljson
from dataclasses import dataclass, asdict

# --- Data Structures for Integration Proofs ---

@dataclass
class MerkleProof:
    leaf_hash: str
    path: list

@dataclass
class SignatureRecord:
    pubkey: str
    signature: str

@dataclass
class ProofOfGovernance:
    signatures: List[SignatureRecord]
    policy_id: str

@dataclass
class RemovalMetadata:
    repo_id: str
    branch: str
    commit_before: str
    commit_after: str

@dataclass
class SecretRemovalProof:
    version: str
    secret_scope_hash: str
    secret_hash: str
    root_before: str
    root_after: str
    merkle_proof: MerkleProof
    removal_timestamp: int
    pog: ProofOfGovernance
    metadata: RemovalMetadata

# --- Generation Functions ---

def generate_canonical_vectors():
    """Generates the canonical JSON test vectors."""
    vectors = [
        {"name": "valid_001_simple", "input": {"a": 1, "z": 2}, "canonical": '{"a":1,"z":2}', "expected_verdict": "VALID"},
        {"name": "valid_002_nested", "input": {"outer": {"a": 1, "z": 2}}, "canonical": '{"outer":{"a":1,"z":2}}', "expected_verdict": "VALID"},
        {"name": "valid_003_array", "input": {"items": [3, 1, 2]}, "canonical": '{"items":[3,1,2]}', "expected_verdict": "VALID"},
        {"name": "valid_004_empty", "input": {}, "canonical": '{}', "expected_verdict": "VALID"},
        {"name": "valid_005_unicode", "input": {"emoji": "🔐", "text": "NoctHub"}, "canonical": '{"emoji":"🔐","text":"NoctHub"}', "expected_verdict": "VALID"},
        {"name": "invalid_001_whitespace", "input_non_canonical": '{ "a": 1 }', "canonical": '{"a":1}', "expected_verdict": "INVALID", "reason": "Contains whitespace"},
        {"name": "invalid_002_wrong_order", "input_non_canonical": '{"z":1,"a":2}', "canonical": '{"a":2,"z":1}', "expected_verdict": "INVALID", "reason": "Wrong key order"},
        {"name": "invalid_003_escaped", "input_non_canonical": '{"key":"value\\u0041"}', "canonical": '{"key":"valueA"}', "expected_verdict": "INVALID", "reason": "Unnecessary escape"},
    ]
    for v in vectors:
        filepath = os.path.join("golden_vectors", "canonical", f"{v['name']}.json")
        os.makedirs(os.path.dirname(filepath), exist_ok=True)
        with open(filepath, "wb") as f:
            f.write(canonicaljson.encode_canonical_json(v))
        print(f"  [OK] Wrote {v['name']}.json")

def generate_bls_vectors():
    """Generates and writes the BLS golden vectors."""
    from nocthub_verifier.constitutional.message import construct_constitutional_message, ActionType
    sk1 = PrivateKey.from_bytes(b"\x00" * 31 + b"\x01")
    sk2 = PrivateKey.from_bytes(b"\x00" * 31 + b"\x02")
    sk3 = PrivateKey.from_bytes(b"\x00" * 31 + b"\x03")
    sk_wrong = PrivateKey.from_bytes(b"\x00" * 31 + b"\x99")
    vectors = [
        {"name": "valid_001_single", "scope": "test-scope-1", "timestamp": 1700000000, "pks": [sk1.get_g1()], "signers": [sk1], "agg_type": "single", "expected_verdict": "VALID"},
        {"name": "valid_002_aggregate_2", "scope": "test-scope-2", "timestamp": 1700000001, "pks": [sk1.get_g1(), sk2.get_g1()], "signers": [sk1, sk2], "agg_type": "aggregate", "expected_verdict": "VALID"},
        {"name": "valid_003_aggregate_3", "scope": "test-scope-3", "timestamp": 1700000002, "pks": [sk1.get_g1(), sk2.get_g1(), sk3.get_g1()], "signers": [sk1, sk2, sk3], "agg_type": "aggregate", "expected_verdict": "VALID"},
        {"name": "invalid_001_wrong_message", "scope": "test-scope-1", "timestamp": 1700000000, "pks": [sk1.get_g1()], "signers": [sk1], "sign_scope": "a-different-scope", "agg_type": "single", "expected_verdict": "INVALID"},
        {"name": "invalid_002_wrong_pubkey", "scope": "test-scope-1", "timestamp": 1700000000, "pks": [sk_wrong.get_g1()], "signers": [sk1], "agg_type": "single", "expected_verdict": "INVALID"},
        {"name": "invalid_003_corrupted", "scope": "test-scope-1", "timestamp": 1700000000, "pks": [sk1.get_g1()], "signers": [sk1], "agg_type": "corrupted", "expected_verdict": "INVALID"},
    ]
    for v in vectors:
        message_to_verify = construct_constitutional_message(ActionType.FORGET, v["scope"], v["timestamp"])
        message_to_sign = construct_constitutional_message(ActionType.FORGET, v.get("sign_scope", v["scope"]), v["timestamp"])
        pk_hex_list = [bytes(pk).hex() for pk in v["pks"]]
        signature_field = {}
        if v["agg_type"] == "single":
            sig = AugSchemeMPL.sign(v["signers"][0], message_to_sign)
            signature_field["single"] = bytes(sig).hex()
        elif v["agg_type"] == "aggregate":
            sigs = [AugSchemeMPL.sign(sk, message_to_sign) for sk in v["signers"]]
            agg_sig = AugSchemeMPL.aggregate(sigs)
            signature_field["aggregate"] = bytes(agg_sig).hex()
        elif v["agg_type"] == "corrupted":
            sig = AugSchemeMPL.sign(v["signers"][0], message_to_sign)
            sig_bytes = bytearray(bytes(sig))
            sig_bytes[0] ^= 0xFF
            signature_field["corrupted"] = bytes(sig_bytes).hex()
        output = {
            "name": v["name"],
            "description": v.get("description", ""),
            "message": binascii.b2a_base64(message_to_verify).decode("utf-8").strip(),
            "message_text": message_to_verify.decode("utf-8"),
            "public_keys": pk_hex_list,
            "signature": signature_field,
            "expected_verdict": v["expected_verdict"],
            "reason": v.get("reason"),
        }
        filepath = os.path.join("golden_vectors", "bls", f"{v['name']}.json")
        os.makedirs(os.path.dirname(filepath), exist_ok=True)
        with open(filepath, "wb") as f:
            f.write(canonicaljson.encode_canonical_json(output))
        print(f"  [OK] Wrote {v['name']}.json")

def generate_integration_vectors():
    """Generates the SecretRemovalProof integration test vectors."""
    sk1 = PrivateKey.from_bytes(b"\x00" * 31 + b"\x01")
    sk2 = PrivateKey.from_bytes(b"\x00" * 31 + b"\x02")

    base_proof = SecretRemovalProof(
        version="1.0",
        secret_scope_hash=binascii.b2a_base64(b"scope_hash_data").decode().strip(),
        secret_hash=binascii.b2a_base64(b"secret_hash_data").decode().strip(),
        root_before=binascii.b2a_base64(b"root_before_data").decode().strip(),
        root_after=binascii.b2a_base64(b"root_after_data").decode().strip(),
        merkle_proof=MerkleProof(leaf_hash="leaf", path=[]),
        removal_timestamp=1700000000,
        pog=ProofOfGovernance(signatures=[], policy_id="policy-123"),
        metadata=RemovalMetadata("repo-abc", "main", "commit-1", "commit-2")
    )

    def get_signed_pog(proof_dict, signers):
        proof_to_sign = proof_dict.copy()
        del proof_to_sign["pog"]
        message = canonicaljson.encode_canonical_json(proof_to_sign)
        sigs = [AugSchemeMPL.sign(sk, message) for sk in signers]
        return ProofOfGovernance(
            signatures=[SignatureRecord(pubkey=bytes(sk.get_g1()).hex(), signature=bytes(sig).hex()) for sk, sig in zip(signers, sigs)],
            policy_id="policy-123"
        )

    valid_proof = asdict(base_proof)
    valid_proof["pog"] = asdict(get_signed_pog(valid_proof, [sk1, sk2]))

    invalid_scope_proof = asdict(base_proof)
    signed_pog_wrong_scope = get_signed_pog(invalid_scope_proof, [sk1, sk2])
    invalid_scope_proof["secret_scope_hash"] = "corrupted"
    invalid_scope_proof["pog"] = asdict(signed_pog_wrong_scope)

    insufficient_sigs_proof = asdict(base_proof)
    insufficient_sigs_proof["pog"] = asdict(get_signed_pog(insufficient_sigs_proof, [sk1]))

    merkle_mismatch_proof = asdict(base_proof)
    merkle_mismatch_proof["pog"] = asdict(get_signed_pog(merkle_mismatch_proof, [sk1, sk2]))
    merkle_mismatch_proof["root_after"] = "corrupted"

    vectors = [
        {"name": "valid_001_complete", "proof": valid_proof, "expected_verdict": "VALID"},
        {"name": "invalid_001_scope", "proof": invalid_scope_proof, "expected_verdict": "INVALID"},
        {"name": "invalid_002_insufficient_sigs", "proof": insufficient_sigs_proof, "expected_verdict": "INVALID"},
        {"name": "invalid_003_merkle", "proof": merkle_mismatch_proof, "expected_verdict": "INVALID"},
    ]

    for v in vectors:
        filepath = os.path.join("golden_vectors", "integration", f"{v['name']}.json")
        os.makedirs(os.path.dirname(filepath), exist_ok=True)
        output = {"name": v["name"], "proof": v["proof"], "expected_verdict": v["expected_verdict"]}
        with open(filepath, "wb") as f:
            f.write(canonicaljson.encode_canonical_json(output))
        print(f"  [OK] Wrote {v['name']}.json")

def update_manifest():
    """Calculates hashes and sizes and updates the manifest."""
    manifest = {
        "version": "2.0",
        "constitutional_article": "Artigo 10 (Mensagem Canônica)",
        "generated_by": "nocthub-golden-v2",
        "purpose": "Ground truth for Python verifier (P1 invariant)",
        "vectors": {},
    }
    golden_vectors_dir = "golden_vectors"
    categories = ["canonical", "bls", "integration"]
    for category in categories:
        manifest["vectors"][category] = {}
        category_dir = os.path.join(golden_vectors_dir, category)
        if not os.path.isdir(category_dir):
            os.makedirs(category_dir, exist_ok=True)
        for filename in sorted(os.listdir(category_dir)):
            if filename.endswith(".json"):
                filepath = os.path.join(category_dir, filename)
                with open(filepath, "rb") as f:
                    content = f.read()
                    sha256_hash = hashlib.sha256(content).hexdigest()
                    size_bytes = len(content)
                manifest["vectors"][category][filename] = {
                    "sha256": sha256_hash,
                    "size_bytes": size_bytes,
                }
    manifest_path = os.path.join(golden_vectors_dir, "MANIFEST.json")
    with open(manifest_path, "wb") as f:
        f.write(canonicaljson.encode_canonical_json(manifest))
    print("  [OK] Wrote MANIFEST.json")

if __name__ == "__main__":
    print("--- Generating Canonical Golden Vectors ---")
    generate_canonical_vectors()
    print("--- Generating BLS Golden Vectors ---")
    generate_bls_vectors()
    print("--- Generating Integration Golden Vectors ---")
    generate_integration_vectors()
    print("--- Updating Manifest ---")
    update_manifest()
    print("--- Generation Complete ---")