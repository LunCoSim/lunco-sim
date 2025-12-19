"""
Artigo 10: Formato Canônico de Mensagem
Constituição NoctHub V1.0
"""

import hashlib
from typing import Tuple
from enum import Enum

class ActionType(Enum):
    FORGET = "FORGET"

class ValidationError(Exception):
    """Erro de validação de mensagem constitucional"""
    pass

def construct_constitutional_message(
    action: ActionType,
    scope: str,
    timestamp: int,
) -> bytes:
    """
    Constrói mensagem constitucional canônica

    ENFORCEMENT: Artigo 10, §1-4
    INVARIANTE: Byte-identical com implementação Rust
    """
    # Validação: apenas FORGET suportado em V1.0
    assert action == ActionType.FORGET, "Only FORGET action supported"

    # §3: Derivação de scope hash
    scope_bytes = scope.encode('utf-8')
    scope_hash = hashlib.sha256(scope_bytes).hexdigest()

    # §1: Formato canônico
    # §2: lowercase hex obrigatório
    # §4: decimal sem padding
    message = f"FORGET:{scope_hash}:{timestamp}"

    # §2: UTF-8 encoding
    return message.encode('utf-8')

def validate_constitutional_message(message: bytes) -> Tuple[str, int]:
    """
    Valida mensagem constitucional

    ENFORCEMENT: Artigo 10, §2-4

    Returns:
        (scope_hash_hex, timestamp)

    Raises:
        ValidationError: se mensagem inválida
    """
    # §2: Deve ser UTF-8 válido
    try:
        message_str = message.decode('utf-8')
    except UnicodeDecodeError:
        raise ValidationError("Invalid UTF-8 encoding")

    # §1: Formato esperado
    parts = message_str.split(':')
    if len(parts) != 3:
        raise ValidationError("Invalid format: expected 3 parts")

    # §1: Prefix
    if parts[0] != "FORGET":
        raise ValidationError("Invalid prefix")

    # §2: Hash deve ser lowercase hex de 64 chars
    scope_hash_hex = parts[1]
    if len(scope_hash_hex) != 64:
        raise ValidationError("Invalid hash length")
    if not all(c in '0123456789abcdef' for c in scope_hash_hex):
        raise ValidationError("Invalid hash format (must be lowercase hex)")

    # §4: Timestamp deve ser decimal válido
    try:
        timestamp = int(parts[2])
    except ValueError:
        raise ValidationError("Invalid timestamp")

    # §4: Sem zeros à esquerda (exceto "0" sozinho)
    if len(parts[2]) > 1 and parts[2][0] == '0':
        raise ValidationError("Leading zeros not allowed")

    # §4: Positivo
    if timestamp < 0:
        raise ValidationError("Timestamp must be non-negative")

    return (scope_hash_hex, timestamp)

# ============================================================================
# TESTES (P1: Fidelidade Rust↔Python)
# ============================================================================

def test_construct_canonical():
    """Python deve gerar mesma mensagem que Rust"""
    msg = construct_constitutional_message(
        ActionType.FORGET,
        "test",
        1700000000,
    )

    expected = b"FORGET:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08:1700000000"
    assert msg == expected

def test_validate_canonical():
    """Python deve aceitar mensagens válidas"""
    msg = b"FORGET:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08:1700000000"

    hash_hex, timestamp = validate_constitutional_message(msg)
    assert hash_hex == "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
    assert timestamp == 1700000000

def test_reject_uppercase():
    """Python deve rejeitar uppercase"""
    msg = b"FORGET:9F86D081884C7D659A2FEAA0C55AD015A3BF4F1B2B0B822CD15D6C15B0F00A08:1700000000"

    try:
        validate_constitutional_message(msg)
        assert False, "Should have raised ValidationError"
    except ValidationError:
        pass  # esperado

def test_unicode_scope():
    """Python deve tratar Unicode identicamente ao Rust"""
    msg = construct_constitutional_message(
        ActionType.FORGET,
        "api-key-🔐",
        0,
    )

    # Scope hash deve ser de UTF-8 encoding
    scope_hash = hashlib.sha256("api-key-🔐".encode('utf-8')).hexdigest()
    expected = f"FORGET:{scope_hash}:0".encode('utf-8')

    assert msg == expected