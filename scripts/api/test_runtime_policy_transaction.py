#!/usr/bin/env python3
"""Drive authored atomic policy creation/undo checks in an owned production session."""
from __future__ import annotations
import argparse
import hashlib
import json
import os
import time
from pathlib import Path
from runtime import BINARY, ROOT, ProductionSession
from run_editor_scene_test import wait_for_scene, wait_for_scene_time_selection


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--log", type=Path, required=True)
    args = parser.parse_args()
    os.environ.update(LUNCOSIM_ISOLATED_RUN="1", LUNCOSIM_EPHEMERAL_SETTINGS="1")
    os.environ.setdefault("LUNCOSIM_CONFIG", str(args.log.resolve().parent / f"policy-transaction-{args.port}-config"))
    scene = ROOT / "assets/scenes/tests/authored_runtime_contracts.usda"
    original = hashlib.sha256(scene.read_bytes()).hexdigest()
    source = (ROOT / "assets/scenarios/tests/runtime_policy_transaction.rhai").read_text()
    with ProductionSession(args.port, windowed=True,
                           extra_args=["--scene", str(scene), "--no-throttle"],
                           log_path=args.log) as session:
        proc = Path(f"/proc/{session.process.pid}")
        if proc.exists() and (proc.joinpath("cwd").resolve() != ROOT or proc.joinpath("exe").resolve() != BINARY.resolve()):
            raise RuntimeError("session does not match the checkout and production binary")
        print(f"OWNED pid={session.process.pid} port={session.port}", flush=True)
        wait_for_scene(session, str(scene), 60)
        wait_for_scene_time_selection(session, args.log, 60)

        def command(name, **params):
            result = session.post({"type": "ExecuteCommand", "command": name, "params": params})
            if result.get("error"):
                raise RuntimeError(result)
            return result.get("data", {})

        def rhai(code):
            return command("RunRhai", code=source + "\n" + code)["stdout"].strip()

        def wait(predicate, label):
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                if predicate():
                    return
                time.sleep(.1)
            raise RuntimeError(f"timed out waiting for {label}; log={args.log}")

        doc = next(item["doc_id"] for item in command("ListOpenDocuments")["open_documents"]
                   if item["kind"] == "usd" and item.get("origin", {}).get("path") == str(scene))
        path = "/AuthoredRuntimeContracts/Policies/AtomicSurface"
        rhai("install_atomic_surface_policy();")
        wait(lambda: rhai("print(atomic_surface_policy_ready());").splitlines()[-1] == "true", "complete policy admission")
        wait(lambda: command("ReadExposures", surface="camera-status")["surfaces"]["camera-status"]["properties"].get("atomic_marker") == "complete", "surface publication")
        for function, expected in [("verify_atomic_surface_policy", 6), ("verify_atomic_surface_policy_undo", 4)]:
            if function.endswith("_undo"):
                command("UndoDocument", doc_id=doc)
                wait(lambda: rhai(f"print(atomic_surface_policy_removed({doc}, {json.dumps(path)}));").splitlines()[-1] == "true", "one-step policy undo")
                wait(lambda: rhai("print(atomic_surface_policy_ready());").splitlines()[-1] == "false", "policy implementation retirement")
            output = rhai(f"{function}({doc}, {json.dumps(path)});")
            print(output, flush=True)
            if f"TESTS_OK {expected}" not in output or "TESTS_FAIL" in output:
                raise RuntimeError("authored policy transaction verdict failed")
    if hashlib.sha256(scene.read_bytes()).hexdigest() != original:
        raise RuntimeError("policy gate changed its authored source fixture")
    print("PASS — ten authored policy transaction checks; fixture unchanged; API Exit")


if __name__ == "__main__":
    main()
