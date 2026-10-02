#!/usr/bin/env python3
"""Drive Twin replacement in a windowed production host; Rhai owns verdicts."""

from __future__ import annotations

import json
import os
import subprocess
import time

from runtime import BINARY, ProductionSession, ROOT

PORT = int(os.environ.get("LUNCOSIM_API_PORT", "4187"))
SOURCE = ROOT / "assets/scenarios/tests/twin_session_retirement.rhai"
TWIN_A = ROOT / "assets/scenes/tests/twin_session_retirement"
TWIN_B = TWIN_A / "replacement"


def execute(session: ProductionSession, command: str, **params) -> dict:
    response = session.post({"type": "ExecuteCommand", "command": command, "params": params})
    if response.get("error"):
        raise RuntimeError(f"{command}: {response}")
    return response.get("data", {})


def documents(session: ProductionSession) -> list[dict]:
    return execute(session, "ListOpenDocuments")["open_documents"]


def wait_for(session: ProductionSession, predicate, detail: str):
    deadline = time.monotonic() + 90
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        if session.process is None or session.process.poll() is not None:
            raise RuntimeError(f"production process exited while waiting for {detail}")
        time.sleep(0.25)
    raise RuntimeError(f"timed out waiting for {detail}; log={session.log_path}")


def open_twin(session: ProductionSession, root, previous_ids=()) -> list[int]:
    execute(session, "OpenTwin", path=str(root))
    def admitted():
        docs = documents(session)
        paths = [str(doc.get("origin", {}).get("path", "")).replace("\\", "/") for doc in docs]
        ids = [doc["doc_id"] for doc in docs]
        if any(str(root).replace("\\", "/") in path for path in paths) and not set(ids).intersection(previous_ids):
            return ids
        return None
    return wait_for(session, admitted, f"fresh documents for {root.name}")


def verdict(function: str, *args) -> None:
    invocation = function + "(" + ",".join(json.dumps(arg) for arg in args) + ")"
    code = SOURCE.read_text(encoding="utf-8") + "\nif !" + invocation + ' { throw "Twin lifecycle regression"; }'
    result = subprocess.run(
        [str(BINARY), "rhai", "--api", str(PORT), "--stdout", "-e", code],
        cwd=ROOT, capture_output=True, text=True, timeout=30,
    )
    print(result.stdout.strip())
    if result.returncode or "TESTS_OK" not in result.stdout or "TESTS_FAIL" in result.stdout:
        raise RuntimeError(f"authored Rhai verdict failed: {result.stdout}\n{result.stderr}")


def main() -> None:
    with ProductionSession(PORT, windowed=True, log_path=ROOT / "target/twin-session-retirement.log") as session:
        ids_a = open_twin(session, TWIN_A)
        execute(session, "ActivatePerspective", id="editor")
        execute(session, "OpenFile", path=str(TWIN_A / "twin_session_retirement.usda"))
        scene_doc = next(doc["doc_id"] for doc in documents(session) if doc["kind"] == "usd")
        execute(session, "OpenUsdPreview", preview=9001, doc_id=scene_doc, edit_target="@root@")
        execute(session, "OpenUsdPreview", preview=9002, doc_id=scene_doc, edit_target="@root@")
        wait_for(session, lambda: execute(session, "InspectUsdViewport")["preview_count"] >= 2,
                 "both Twin A editor previews")
        execute(session, "CreateScratchModelicaDocument", name="TwinRetirementScratch",
                source="model TwinRetirementScratch Real x; equation x = 1; end TwinRetirementScratch;")
        wait_for(session, lambda: any(doc["kind"] == "modelica" for doc in documents(session)), "Modelica scratch")
        ids_a = [doc["doc_id"] for doc in documents(session)]
        ids_b = open_twin(session, TWIN_B, ids_a)
        verdict("verify_editor_retirement", ids_a, ids_b)

        # Same-path reopening is a fresh session, even when no scene path changes.
        ids_reload = open_twin(session, TWIN_B, ids_b)
        verdict("verify_editor_retirement", ids_b, ids_reload)

        # Invalid candidates must leave the admitted session untouched.
        session.post({"type": "ExecuteCommand", "command": "OpenTwin",
                      "params": {"path": str(ROOT / "target/no-such-twin-retirement-negative")}})
        verdict("verify_session_preserved", ids_reload)

        # Repeated replacement cannot accumulate source documents or previews.
        ids_return = open_twin(session, TWIN_A, ids_reload)
        verdict("verify_editor_retirement", ids_reload, ids_return)
        execute(session, "ClearScene")
        for doc in documents(session):
            execute(session, "CloseDocument", doc_id=doc["doc_id"])
        wait_for(session, lambda: not documents(session), "test document cleanup before Exit")
        verdict("verify_editor_retirement", ids_return, [])
        print(f"PASS: Twin switching, same-Twin reopening, rejected replacement; owned PID={session.process.pid}, port={PORT}")
    print("API Exit completed; owned process and port released")


if __name__ == "__main__":
    main()
