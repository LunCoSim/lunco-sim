#!/usr/bin/env python3
"""Capture shared asset resource UI in an owned production session.

Rhai owns observable source/scope assertions. Images and resource diagnostics
are captured for directed visual inspection; this is not browser acceptance.
"""
from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess
import time

from runtime import BINARY, ProductionSession, ROOT, assert_render_log_clean, get_json, port_is_open

PORT = int(os.environ.get("LUNCOSIM_API_PORT", "4286"))
OUTPUT = ROOT / ".lunco/asset-resource-review"
ASSERTIONS = ROOT / "assets/scenarios/tests/asset_resolution.rhai"


def execute(session, command, **params):
    result = session.post({"type": "ExecuteCommand", "command": command, "params": params})
    if result.get("error"):
        raise RuntimeError(f"{command}: {result}")
    return result.get("data", {})


def verdict(function, *args):
    code = ASSERTIONS.read_text() + "\n" + function + "(" + ",".join(json.dumps(arg) for arg in args) + ");"
    result = subprocess.run([str(BINARY), "rhai", "--api", str(PORT), "--stdout", "-e", code],
                            cwd=ROOT, capture_output=True, text=True, timeout=30)
    print(result.stdout.strip(), flush=True)
    if result.returncode or "TESTS_OK" not in result.stdout or "TESTS_FAIL" in result.stdout:
        raise RuntimeError(f"Rhai verdict failed: {result.stdout}\n{result.stderr}")


def wait_document(session, source):
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        for document in execute(session, "ListOpenDocuments")["open_documents"]:
            path = document.get("origin", {}).get("path")
            if document["kind"] == "modelica" and path and Path(path) == source:
                status = execute(session, "CompileStatus", doc_id=document["doc_id"])
                if status.get("ast_parsed"):
                    return document["doc_id"]
        time.sleep(.25)
    raise RuntimeError("resource source did not become a parsed document")


def capture(session, name, uri, settle_s=1):
    execute(session, "OpenAssetResource", uri=uri)
    # Capture is visual evidence; loading completion is checked in the image.
    time.sleep(settle_s)
    path = OUTPUT / f"{name}.png"
    session.capture_screenshot(path)
    print(f"captured {name}: {path}", flush=True)


def main():
    image = Path(os.environ["LUNCOSIM_RESOURCE_TEST_IMAGE"]).resolve(strict=True)
    replacement = Path(os.environ["LUNCOSIM_RESOURCE_TEST_REPLACEMENT"]).resolve(strict=True)
    library_uri = os.environ["LUNCOSIM_RESOURCE_LIBRARY_URI"]
    OUTPUT.mkdir(parents=True, exist_ok=True)
    fixture = OUTPUT / "source # % 月"
    resource = fixture / "Nested/Resources/literal # % Ж.png"
    resource.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(image, resource)
    native_image = fixture / ("native:literal # %.png" if os.name == "posix" else "native literal # %.png")
    shutil.copyfile(image, native_image)
    source = fixture / "AssetResourceProbe.mo"
    source.write_text("within;\npackage AssetResourceProbe\n  package Nested\n  end Nested;\nend AssetResourceProbe;\n")
    duplicate = fixture / "other/AssetResourceProbe.mo"
    duplicate.parent.mkdir(exist_ok=True)
    duplicate.write_text(source.read_text())
    twin = OUTPUT / "admitted-package"
    twin_resource = twin / "Nested/Resources/plot.png"
    twin_resource.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(image, twin_resource)
    twin_source = twin / "OwnedAssetProbe.mo"
    twin_source.write_text("within;\npackage OwnedAssetProbe\n  package Nested\n  end Nested;\nend OwnedAssetProbe;\n")
    (twin / "twin.toml").write_text('name = "asset_owner_probe"\nversion = "0.1.0"\n')
    replacement_twin = OUTPUT / "replacement-twin"
    replacement_twin.mkdir(exist_ok=True)
    (replacement_twin / "twin.toml").write_text('name = "asset_owner_replacement"\nversion = "0.1.0"\n')
    uri = "MoDeLiCa://AssetResourceProbe.Nested/Resources/literal%20%23%20%25%20%D0%96.png"
    log = OUTPUT / "production.log"
    with ProductionSession(PORT, windowed=True, log_path=log) as session:
        process = session.process
        assert process is not None
        if Path("/proc").is_dir():
            assert Path(f"/proc/{process.pid}/cwd").resolve() == ROOT
            assert Path(f"/proc/{process.pid}/exe").resolve() == BINARY.resolve()
        schema = get_json(PORT, "/api/commands/schema")
        (OUTPUT / "schema.json").write_text(json.dumps(schema, indent=2))
        diagnostics = []
        for scheme in ["twin", "TwIn"]:
            try:
                response = session.post({"type": "ExecuteCommand", "command": "RunScenarioAsset",
                                         "params": {"source_asset": f"{scheme}://unmounted/probe.rhai"}})
                diagnostics.append(str(response.get("error", "")))
            except RuntimeError as error:
                diagnostics.append(str(error))
        verdict("verify_scheme_scope", *diagnostics)
        execute(session, "OpenFile", path=str(source))
        document = wait_document(session, source)
        verdict("verify_resource_source", document, True)
        capture(session, "package-literal", uri)
        execute(session, "OpenFile", path=str(duplicate))
        other_document = wait_document(session, duplicate)
        verdict("verify_resource_source", other_document, True)
        capture(session, "ambiguous", uri)
        execute(session, "CloseDocument", doc_id=other_document)
        verdict("verify_resource_source", other_document, False)
        capture(session, "package-recovered", uri)
        capture(session, "missing", "modelica://AssetResourceProbe.Nested/Resources/missing.png")
        capture(session, "traversal", "modelica://AssetResourceProbe.Nested/%2e%2e/outside.png")
        capture(session, "file-uri", native_image.as_uri().replace("file:///", "FiLe:/", 1))
        capture(session, "library-pending", library_uri)
        capture(session, "library", library_uri, settle_s=8)
        shutil.copyfile(replacement, native_image)
        capture(session, "native-replacement", native_image.as_uri().replace("file:///", "FiLe:/", 1))
        execute(session, "CloseDocument", doc_id=document)
        verdict("verify_resource_source", document, False)
        capture(session, "retired", uri)
        shutil.copyfile(replacement, resource)
        execute(session, "OpenFile", path=str(source))
        document = wait_document(session, source)
        verdict("verify_resource_source", document, True)
        capture(session, "replacement", uri)
        execute(session, "OpenTwin", path=str(twin))
        execute(session, "OpenFile", path=str(twin_source))
        twin_document = wait_document(session, twin_source)
        verdict("verify_resource_source", twin_document, True)
        twin_uri = "modelica://OwnedAssetProbe.Nested/Resources/plot.png"
        capture(session, "twin-document-view", twin_uri)
        execute(session, "CloseDocument", doc_id=twin_document)
        verdict("verify_resource_source", twin_document, False)
        capture(session, "twin-document-closed", twin_uri)
        execute(session, "OpenTwin", path=str(replacement_twin))
        capture(session, "twin-retired", twin_uri)
        assert_render_log_clean(log)
    assert not port_is_open(PORT)
    (OUTPUT / "validation.json").write_text(json.dumps({"port": PORT, "exit": 0, "port_released": True,
        "rhai_assertions": 9, "visual_captures": 14}, indent=2))
    print("asset resource production checks completed; inspect the fourteen captures", flush=True)


if __name__ == "__main__":
    main()
