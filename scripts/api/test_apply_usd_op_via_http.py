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


def wait_for_document(session: ProductionSession, title: str) -> int:
    timeout_s = float(os.environ.get("LUNCOSIM_DOCUMENT_TIMEOUT_S", "30"))
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        response = require_success(
            session.post({"type": "ListOpenDocuments"}), "ListOpenDocuments"
        )
        for document in response.get("data", {}).get("open_documents", []):
            if document.get("title") == title:
                return int(document["doc_id"])
        time.sleep(0.25)
    raise RuntimeError(f"document {title!r} was not opened within {timeout_s:g}s")


def main() -> None:
    scaffold = """#usda 1.0
(
    defaultPrim = "World"
)

def Xform "World"
{
}
"""

    with tempfile.TemporaryDirectory(prefix="luncosim-api-") as directory:
        path = Path(directory) / "test_http_usd.usda"
        path.write_text(scaffold, encoding="utf-8")
        port = int(os.environ.get("LUNCOSIM_API_PORT", "4101"))

        print(f"📝 Created USDA scaffold at {path}")
        print(f"🚀 Starting production luncosim with API on port {port}...")
        with ProductionSession(port) as session:
            require_success(
                session.post({"type": "OpenFile", "path": str(path)}), "OpenFile"
            )
            doc_id = wait_for_document(session, path.name)
            print(f"✅ Opened document {doc_id} through the API.")

            require_success(
                session.post(
                    {
                        "type": "ExecuteCommand",
                        "command": "ApplyUsdOp",
                        "params": {
                            "doc": doc_id,
                            "op": {
                                "AddPrim": {
                                    "edit_target": "@root@",
                                    "parent_path": "/World",
                                    "name": "TestCube",
                                    "type_name": "Cube",
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
                            "doc": doc_id,
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
                        "command": "SaveDocument",
                        "params": {"doc": doc_id},
                    }
                ),
                "SaveDocument",
            )

        content = path.read_text(encoding="utf-8")
        if 'def Cube "TestCube"' not in content or "double size = 7.5" not in content:
            raise RuntimeError(f"saved USDA did not contain the authored cube: {content}")
        print("✅ ApplyUsdOp and SaveDocument persisted the authored USD change.")


if __name__ == "__main__":
    main()
