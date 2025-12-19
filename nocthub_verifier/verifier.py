"""
📜 NOCTURNE OFFLINE VERIFIER v1.0
Implementação de referência conforme Especificação Constitucional v1.0.
Foco: Fidelidade Rust (P1), Determinismo (P3) e Verificação BLS Explicita (P5).
"""

import json
import binascii
import os
from typing import Dict, Any, List
from dataclasses import dataclass

import canonicaljson
from blspy import AugSchemeMPL, G1Element, G2Element

@dataclass
class VerificationResult:
    is_valid: bool
    reason: str
    code: str

class NocturneConstitutionalVerifier:
    """
    Verificador determinístico de Golden Vectors v2.
    Pipeline: CanonicalJSON -> Schema -> BLS Signature -> Constitutional Rules
    """
    MIN_SIGNATURES_FOR_SRP = 2

    def to_canonical(self, data: Dict[str, Any]) -> bytes:
        try:
            return canonicaljson.encode_canonical_json(data)
        except Exception as e:
            raise ValueError(f"Falha na canonicização RFC 8785: {str(e)}")

    def verify_bls_aggregate(self, message: bytes, sig_hex: str, pk_hex_list: List[str]) -> bool:
        """Verifica uma assinatura BLS agregada."""
        try:
            if not pk_hex_list:
                return False
            pks = [G1Element.from_bytes(binascii.unhexlify(pk_hex)) for pk_hex in pk_hex_list]
            agg_sig = G2Element.from_bytes(binascii.unhexlify(sig_hex))
            return AugSchemeMPL.aggregate_verify(pks, [message] * len(pks), agg_sig)
        except:
            return False

    def verify_bls_single(self, message: bytes, sig_hex: str, pk_hex: str) -> bool:
        """Verifica uma única assinatura BLS."""
        try:
            pk = G1Element.from_bytes(binascii.unhexlify(pk_hex))
            sig = G2Element.from_bytes(binascii.unhexlify(sig_hex))
            return AugSchemeMPL.verify(pk, message, sig)
        except:
            return False

    def verify_canonical_vector(self, vector: Dict[str, Any]) -> VerificationResult:
        """Verifica um vetor de teste da categoria 'canonical'."""
        is_valid_case = vector["expected_verdict"] == "VALID"
        if is_valid_case:
            input_data = vector["input"]
            canonical_bytes = self.to_canonical(input_data)
            if canonical_bytes.decode("utf-8") == vector["canonical"]:
                return VerificationResult(True, "OK", "CANONICAL_MATCH")
            else:
                return VerificationResult(False, "Canonical representation mismatch", "CANONICAL_MISMATCH")
        else:  # INVALID case
            input_str = vector["input_non_canonical"]
            try:
                parsed = json.loads(input_str)
                canonical_bytes = self.to_canonical(parsed)
                if input_str.encode("utf-8") != canonical_bytes:
                    return VerificationResult(True, "OK: Non-canonical input correctly identified", "NON_CANONICAL_DETECTED")
                else:
                    return VerificationResult(False, "Non-canonical input was already canonical", "NON_CANONICAL_FAILURE")
            except Exception as e:
                return VerificationResult(False, f"Error processing non-canonical input: {e}", "CANONICAL_PROCESSING_ERROR")

    def verify_bls_vector(self, vector: Dict[str, Any]) -> VerificationResult:
        """Verifica um vetor de teste da categoria 'bls'."""
        message = binascii.a2b_base64(vector["message"])
        public_keys = vector["public_keys"]
        signature_field = vector["signature"]
        crypto_verdict = False
        if "single" in signature_field:
            crypto_verdict = self.verify_bls_single(message, signature_field["single"], public_keys[0])
        elif "aggregate" in signature_field:
            crypto_verdict = self.verify_bls_aggregate(message, signature_field["aggregate"], public_keys)
        elif "corrupted" in signature_field:
            crypto_verdict = self.verify_bls_single(message, signature_field["corrupted"], public_keys[0])

        expected_valid = vector["expected_verdict"] == "VALID"
        if crypto_verdict == expected_valid:
            return VerificationResult(True, "OK", "BLS_VERDICT_MATCH")
        else:
            return VerificationResult(False, f"BLS verification failed. Expected verdict '{vector['expected_verdict']}', crypto result was '{crypto_verdict}'", "BLS_VERDICT_MISMATCH")

    def verify_integration_vector(self, vector: Dict[str, Any]) -> VerificationResult:
        """Verifica um vetor de teste da categoria 'integration'."""
        proof_data = vector["proof"]
        pog = proof_data["pog"]

        if len(pog["signatures"]) < self.MIN_SIGNATURES_FOR_SRP:
            crypto_verdict = False
            reason = f"Constitutional violation: Insufficient signatures (found {len(pog['signatures'])}, require {self.MIN_SIGNATURES_FOR_SRP})"
            code = "INSUFFICIENT_SIGNATURES"
        else:
            signed_data = proof_data.copy()
            del signed_data["pog"]
            message_to_verify = self.to_canonical(signed_data)

            pubkeys_hex = [rec["pubkey"] for rec in pog["signatures"]]
            signatures_hex = [rec["signature"] for rec in pog["signatures"]]

            try:
                signatures = [G2Element.from_bytes(binascii.unhexlify(s_hex)) for s_hex in signatures_hex]
                agg_sig = AugSchemeMPL.aggregate(signatures)
                agg_sig_hex = bytes(agg_sig).hex()
                crypto_verdict = self.verify_bls_aggregate(message_to_verify, agg_sig_hex, pubkeys_hex)
                reason = f"Crypto verification result: {crypto_verdict}"
                code = "CRYPTO_VERDICT"
            except Exception as e:
                crypto_verdict = False
                reason = f"Error during crypto verification: {e}"
                code = "CRYPTO_FAILURE"

        expected_valid = vector["expected_verdict"] == "VALID"
        if crypto_verdict == expected_valid:
            return VerificationResult(True, "OK", "INTEGRATION_VERDICT_MATCH")
        else:
            return VerificationResult(False, f"Integration verification failed. Expected '{vector['expected_verdict']}'. Reason: {reason}", code if not crypto_verdict else "INTEGRATION_VERDICT_MISMATCH")

def main():
    verifier = NocturneConstitutionalVerifier()
    manifest_path = os.path.join("golden_vectors", "MANIFEST.json")
    with open(manifest_path, "r") as f:
        manifest = json.load(f)
    total_tests = 0
    passed_tests = 0
    for category, files in manifest["vectors"].items():
        print(f"--- Verifying category: {category} ---")
        for filename in sorted(files.keys()):
            total_tests += 1
            vector_path = os.path.join("golden_vectors", category, filename)
            with open(vector_path, "rb") as f:
                vector_bytes = f.read()
                vector = json.loads(vector_bytes)
            result = VerificationResult(False, "Category not implemented", "NOT_IMPLEMENTED")
            if category == "canonical":
                result = verifier.verify_canonical_vector(vector)
            elif category == "bls":
                result = verifier.verify_bls_vector(vector)
            elif category == "integration":
                result = verifier.verify_integration_vector(vector)
            if result.is_valid:
                print(f"  [PASS] {filename}")
                passed_tests += 1
            else:
                print(f"  [FAIL] {filename}: {result.reason} ({result.code})")
    print("\n--- Summary ---")
    print(f"  {passed_tests}/{total_tests} tests passed.")
    if passed_tests == total_tests:
        print("  ✅ All tests passed!")
    else:
        print(f"  ❌ {total_tests - passed_tests} tests failed.")

if __name__ == "__main__":
    main()