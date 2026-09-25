#!/usr/bin/env python3
"""Exercise the live USD authoring API against the production binary."""

from __future__ import annotations

import tempfile
import os
import time
from pathlib import Path

from runtime import ProductionSession


def require_success(response: dict, operation: str) -> dict:
    if response.get("error"):
        raise RuntimeError(f"{operation} failed: {response}")
    return response


def wait_for_new_usd_document(session: ProductionSession) -> int:
    timeout_s = float(os.environ.get("LUNCOSIM_DOCUMENT_TIMEOUT_S", "30"))
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        response = require_success(
            session.post(
                {"type": "ExecuteCommand", "command": "ListOpenDocuments", "params": {}}
            ),
            "ListOpenDocuments",
        )
        for document in response.get("data", {}).get("open_documents", []):
            origin = document.get("origin", {})
            if document.get("kind") == "usd" and origin.get("kind") == "untitled":
                return int(document["doc_id"])
        time.sleep(0.25)
    raise RuntimeError(f"new untitled USD document was not created within {timeout_s:g}s")


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="luncosim-api-") as directory:
        path = Path(directory) / "test_http_usd.usda"
        port = int(os.environ.get("LUNCOSIM_API_PORT", "4101"))

        print(f"🚀 Starting production luncosim with API on port {port}...")
        with ProductionSession(port) as session:
            require_success(
                session.post(
                    {
                        "type": "ExecuteCommand",
                        "command": "NewDocument",
                        "params": {"kind": "usd"},
                    }
                ),
                "NewDocument(usd)",
            )
            doc_id = wait_for_new_usd_document(session)
            print(f"✅ Created untitled USD document {doc_id} through the API.")

            require_success(
                session.post(
                    {
                        "type": "ExecuteCommand",
                        "command": "ApplyUsdOp",
                        "params": {
                            "doc_id": doc_id,
                            "parent_gen": 0,
                            "op": {
                                "AddPrim": {
                                    "edit_target": "@root@",
                                    "parent_path": "/World",
                                    "name": "TestCube",
                                    "type_name": "Cube",
                                    "reference": None,
                                    "reference_prim_path": None,
                                }
                            },
                        },
                    }
                ),
                "ApplyUsdOp(AddPrim)",
            )
            require_success(
                session.post(
                    {
                        "type": "ExecuteCommand",
                        "command": "ApplyUsdOp",
                        "params": {
                            "doc_id": doc_id,
                            "op": {
                                "SetAttribute": {
                                    "edit_target": "@root@",
                                    "path": "/World/TestCube",
                                    "name": "size",
                                    "type_name": "double",
                                    "value": "7.5",
                                }
                            },
                        },
                    }
                ),
                "ApplyUsdOp(SetAttribute)",
            )
            require_success(
                session.post(
                    {
                        "type": "ExecuteCommand",
                        "command": "SaveAsDocument",
                        "params": {"doc_id": doc_id, "path": str(path)},
                    }
                ),
                "SaveAsDocument",
            )

        content = path.read_text(encoding="utf-8")
        if 'def Cube "TestCube"' not in content or "double size = 7.5" not in content:
            raise RuntimeError(f"saved USDA did not contain the authored cube: {content}")
        print("✅ ApplyUsdOp and SaveAsDocument persisted the authored USD change.")


if __name__ == "__main__":
    main()
