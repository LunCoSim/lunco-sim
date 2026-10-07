#!/usr/bin/env python3
"""Exercise native rename/model persistence in an owned production session.

Rhai owns runtime assertions; this driver creates non-USD scratch sources,
sequences public commands, and verifies process/transport lifecycle.
"""
from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import time
import uuid

from runtime import BINARY, ProductionSession, ROOT

PORT = int(os.environ.get("LUNCOSIM_API_PORT", "4193"))
SOURCE = ROOT / "assets/scenarios/tests/path_interoperability.rhai"


def execute(session, command, **params):
    result = session.post({"type": "ExecuteCommand", "command": command, "params": params})
    if result.get("error"):
        raise RuntimeError(f"{command}: {result}")
    return result.get("data", {})


def wait_for(session, predicate, detail):
    deadline = time.monotonic() + 90
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        if session.process.poll() is not None:
            raise RuntimeError(f"production process exited waiting for {detail}")
        time.sleep(0.25)
    raise RuntimeError(f"timed out waiting for {detail}; log={session.log_path}")


def model_document(session, filename):
    for entry in execute(session, "ListOpenDocuments")["open_documents"]:
        path = str(entry.get("origin", {}).get("path", "")).replace("\\", "/")
        if entry["kind"] == "modelica" and path.endswith("/" + filename):
            status = execute(session, "CompileStatus", doc_id=entry["doc_id"])
            if status.get("ast_parsed") and "PortableProbe" in status.get("candidates", []):
                return entry["doc_id"]
    return None


def verdict(function, *args):
    code = SOURCE.read_text(encoding="utf-8") + "\n" + function + "(" + ",".join(json.dumps(arg) for arg in args) + ");"
    result = subprocess.run([str(BINARY), "rhai", "--api", str(PORT), "--stdout", "-e", code],
                            cwd=ROOT, capture_output=True, text=True, timeout=30)
    print(result.stdout.strip())
    if result.returncode or "TESTS_OK" not in result.stdout or "TESTS_FAIL" in result.stdout:
        raise RuntimeError(f"Rhai verdict failed: {result.stdout}\n{result.stderr}")


def literal_model_ready():
    code = SOURCE.read_text(encoding="utf-8") + '\nprint("PATH_PROBE_READY " + literal_model_ready());'
    result = subprocess.run([str(BINARY), "rhai", "--api", str(PORT), "--stdout", "-e", code],
                            cwd=ROOT, capture_output=True, text=True, timeout=30)
    if result.returncode:
        raise RuntimeError(f"Rhai readiness probe failed: {result.stdout}\n{result.stderr}")
    return "PATH_PROBE_READY true" in result.stdout


def usd_document(session, filename=None):
    for entry in execute(session, "ListOpenDocuments")["open_documents"]:
        if entry["kind"] != "usd":
            continue
        origin = entry.get("origin", {})
        if filename is None and origin.get("kind") == "untitled":
            return entry["doc_id"]
        if filename and str(origin.get("path", "")).replace("\\", "/").endswith("/" + filename):
            return entry["doc_id"]
    return None


def exercise_usd_model_source(session, root, model_name):
    execute(session, "NewDocument", kind="usd")
    doc = wait_for(session, lambda: usd_document(session), "new USD document")
    inspection = execute(session, "InspectUsdDocument", doc_id=doc, path="/World")
    if not inspection["prim"]["exists"] or inspection["layers"]["root"]["id"] != "@root@":
        raise RuntimeError(f"unexpected USD edit target: {inspection}")

    def apply(operation, **params):
        execute(session, "ApplyUsdOp", doc_id=doc, op={operation: {"edit_target": "@root@", **params}})

    for name, kind in [("LiteralProbe", "Cube"), ("LiteralModel", "Scope")]:
        apply("AddPrim", parent_path="/World", name=name, type_name=kind,
              reference=None, reference_prim_path=None)
    apply("SetApiSchemas", path="/World/LiteralModel", schemas=["LunCoProgramAPI"])
    for name, kind, value in [("info:implementationSource", "token", '"sourceAsset"'),
                              ("info:sourceAsset", "asset", "@" + model_name + "@"), ("outputs:x", "double", "0")]:
        apply("SetAttribute", path="/World/LiteralModel", name=name, type_name=kind, value=value)
    filename = "scene # % probe.usda"
    path = root / filename
    execute(session, "SaveAsDocument", doc_id=doc, path=str(path))
    execute(session, "CloseDocument", doc_id=doc)
    (root / "twin.toml").write_text('name = "path_interoperability"\nversion = "0.1.0"\n[usd]\ndefault_scene = "' + filename + '"\n', encoding="utf-8")
    execute(session, "OpenTwin", path=str(root))
    wait_for(session, lambda: filename in [file["relative_path"] for file in execute(session, "ListTwin").get("files", [])], "saved USD Twin admission")
    execute(session, "OpenFile", path=str(path))
    reopened = wait_for(session, lambda: usd_document(session, filename), "literal USD source readback")
    execute(session, "InspectUsdDocument", doc_id=reopened, path="/World/LiteralProbe")
    wait_for(session, literal_model_ready, "literal Modelica source equation result")
    # Stable scenes use prepared readers. A typed edit promotes the live stage.
    execute(session, "ApplyUsdOp", doc_id=reopened, op={"SetAttribute": {
        "edit_target": "@root@", "path": "/World/LiteralProbe", "name": "size",
        "type_name": "double", "value": "3"}})
    wait_for(session, lambda: execute(session, "ResolveUsdTarget", doc_id=reopened,
             path="/World/LiteralProbe", edit_target="@root@").get("source") == "canonical_stage",
             "edited canonical USD stage")
    verdict("verify_usd_path", reopened, filename)
    execute(session, "SaveDocument", doc_id=reopened)
    execute(session, "CloseDocument", doc_id=reopened)


def main():
    area = ROOT / "target/path-interoperability" / str(uuid.uuid4())
    root = area / "Twin Мир # %"
    root.mkdir(parents=True)
    original, renamed, case_name = "PortableProbe # %.mo", "Renamed # %.mo", "renamed # %.mo"
    (root / "twin.toml").write_text('name = "path_interoperability"\nversion = "0.1.0"\n', encoding="utf-8")
    (root / original).write_text("model PortableProbe output Real x; equation x = 1; end PortableProbe;\n", encoding="utf-8")
    (root / "occupied.mo").write_text("model Occupied Real y; equation y = 3; end Occupied;\n", encoding="utf-8")
    os.environ["LUNCOSIM_CONFIG"] = str(area / "config")
    os.environ["LUNCOSIM_EPHEMERAL_SETTINGS"] = "1"
    os.environ["LUNCOSIM_ISOLATED_RUN"] = "1"
    with ProductionSession(PORT, windowed=True, log_path=area / "runtime.log") as session:
        pid = session.process.pid
        if Path("/proc").exists():
            assert Path(f"/proc/{pid}/cwd").resolve() == ROOT
            assert Path(f"/proc/{pid}/exe").resolve() == BINARY.resolve()
            listener = subprocess.check_output(["ss", "-ltnp", "sport", "=", f":{PORT}"], text=True)
            assert f"pid={pid}," in listener
        print(f"Owned production PID={pid}, port={PORT}, binary={BINARY}, cwd={ROOT}")
        schema = session.post({"type": "DiscoverSchema"})
        (area / "api-schema.json").write_text(json.dumps(schema, ensure_ascii=False), encoding="utf-8")
        print(f"Runtime schema={area / 'api-schema.json'}")
        execute(session, "OpenTwin", path=str(root))
        wait_for(session, lambda: execute(session, "ListTwin").get("open") is True, "Twin admission")
        execute(session, "OpenFile", path=str(root / original))
        doc = wait_for(session, lambda: model_document(session, original), "literal model source")
        verdict("verify_model_path", doc, original, "x = 1", False)
        root_spelling = str(root.parent) + os.sep + "." + os.sep + root.name
        execute(session, "RenameTwinEntry", twin_root=root_spelling, relative_path=original, new_name=renamed)
        wait_for(session, lambda: model_document(session, renamed), "canonical-root rename")
        verdict("verify_model_path", doc, renamed, "x = 1", False)
        execute(session, "RenameTwinEntry", twin_root=root_spelling, relative_path=renamed, new_name=case_name)
        wait_for(session, lambda: model_document(session, case_name), "case-only rename")
        verdict("verify_model_path", doc, case_name, "x = 1", False)
        for name in ["occupied.mo", "NUL", "name?", "name."]:
            diagnostic = ""
            try:
                response = session.post({"type": "ExecuteCommand", "command": "RenameTwinEntry", "params": {
                    "twin_root": root_spelling, "relative_path": case_name, "new_name": name}})
                diagnostic = str(response.get("error", ""))
            except RuntimeError as error:
                diagnostic = str(error)
            verdict("verify_rename_rejection", doc, case_name, diagnostic)
        if os.name != "nt":
            (root / "occupied-link.mo").symlink_to("occupied.mo")
            diagnostic = ""
            try:
                response = session.post({"type": "ExecuteCommand", "command": "RenameTwinEntry", "params": {
                    "twin_root": root_spelling, "relative_path": "occupied-link.mo", "new_name": "occupied.mo"}})
                diagnostic = str(response.get("error", ""))
            except RuntimeError as error:
                diagnostic = str(error)
            verdict("verify_rename_rejection", doc, case_name, diagnostic)
            (root / "occupied-link.mo").unlink()
        execute(session, "SetDocumentSource", doc_id=doc,
                source="model PortableProbe output Real x; equation x = 2; end PortableProbe;\n")
        wait_for(session, lambda: execute(session, "GetDocumentSource", doc_id=doc).get("source", "").find("x = 2") >= 0, "edited source")
        execute(session, "SaveDocument", doc_id=doc)
        wait_for(session, lambda: execute(session, "GetDocumentSource", doc_id=doc).get("dirty") is False, "model save")
        wait_for(session, lambda: model_document(session, case_name), "edited model parse")
        verdict("verify_model_path", doc, case_name, "x = 2", True)
        execute(session, "CloseDocument", doc_id=doc)
        execute(session, "OpenTwin", path=str(root))
        wait_for(session, lambda: execute(session, "ListTwin").get("open") is True, "Twin readback admission")
        execute(session, "OpenFile", path=str(root / case_name))
        reopened = wait_for(session, lambda: model_document(session, case_name), "saved source readback")
        verdict("verify_model_path", reopened, case_name, "x = 2", True)
        verdict("verify_saved_tree", [original, renamed], case_name)
        execute(session, "CloseDocument", doc_id=reopened)
        exercise_usd_model_source(session, root, case_name)
        print(f"PASS: canonical-root rename, case rename, rejected targets, Modelica save/readback; fixtures={root}")
    print("API Exit completed; owned process and port released")


if __name__ == "__main__":
    main()
