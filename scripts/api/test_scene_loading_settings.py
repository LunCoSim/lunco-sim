#!/usr/bin/env python3
"""Exercise layered scene settings in owned production sessions.

USD fixtures are authored through the live document API. Rhai owns the
observable scene and settings verdicts; this driver sequences the sessions.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import time
import tomllib
import uuid

from runtime import BINARY, ROOT, ProductionSession, wait_for_ready

SOURCE = ROOT / "assets/scenarios/tests/scene_loading_settings.rhai"


def execute(session, command, **params):
    response = session.post({"type": "ExecuteCommand", "command": command, "params": params})
    if response.get("error"):
        raise RuntimeError(f"{command}: {response}")
    return response.get("data", {})


def rejection(session, command, **params):
    try:
        response = session.post({"type": "ExecuteCommand", "command": command, "params": params})
        return str(response.get("error") or "")
    except RuntimeError as error:
        return str(error)


def wait_for(session, predicate, detail, timeout=90):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        if session.process is None or session.process.poll() is not None:
            raise RuntimeError(f"owned process exited waiting for {detail}; log={session.log_path}")
        time.sleep(.15)
    raise RuntimeError(f"timed out waiting for {detail}; log={session.log_path}")


def prim_present(session, path):
    try:
        return bool(execute(session, "QueryUsdPrim", path=path).get("spawned"))
    except RuntimeError as error:
        if any(text in str(error) for text in ("HTTP 404", "no USD stage loaded", "exactly one mounted live stage is required", "not found in the requested document")):
            return False
        raise


def inspect_document(session, doc, path="/World"):
    try:
        return execute(session, "InspectUsdDocument", doc_id=doc, path=path)
    except RuntimeError as error:
        if "USD stage source asset is unavailable" in str(error):
            return None
        raise


def verdict(session, function, *values):
    code = SOURCE.read_text() + "\n" + function + "(" + ",".join("()" if value is None else json.dumps(value) for value in values) + ");"
    output = execute(session, "RunRhai", code=code).get("stdout", "")
    print(output.strip(), flush=True)
    if "TESTS_OK" not in output or "TESTS_FAIL" in output:
        raise RuntimeError(f"Rhai verdict failed: {output}")


def twin_id(session):
    scope = execute(session, "ReadActiveWorkspace")
    return scope.get("scope_id") if scope.get("open") else None


def wait_selection(session, previous, expected, source, marker, require_tool=True):
    stable_since = None
    def ready():
        nonlocal stable_since
        current = twin_id(session)
        if not current or current == previous:
            stable_since = None
            return False
        settings = execute(session, "GetSceneLoadingSettings")
        readiness = execute(session, "GetReadiness")
        if settings.get("admitted") != expected or settings.get("admitted_source") != source:
            stable_since = None
            return False
        if not readiness.get("ready") or readiness.get("world_hold") or readiness.get("faulted"):
            stable_since = None
            return False
        default = prim_present(session, "/World/DefaultMarker")
        explicit = prim_present(session, "/World/ExplicitMarker")
        tools = execute(session, "ListToolLibraries")["libraries"]
        has_tool = any(tool["name"] == "scene_loading_gate" for tool in tools)
        if has_tool != require_tool or (default, explicit) != (marker == "default", marker == "explicit"):
            stable_since = None
            return False
        if stable_since is None:
            stable_since = time.monotonic()
        return current if time.monotonic() - stable_since >= .5 else False
    return wait_for(session, ready, f"{source} selection ({marker})")


def author_scene(session, root, filename, marker, *, runner=False):
    execute(session, "NewDocument", kind="usd")
    def untitled():
        return next((doc["doc_id"] for doc in execute(session, "ListOpenDocuments")["open_documents"]
                     if doc["kind"] == "usd" and doc["origin"].get("kind") == "untitled"), None)
    doc = wait_for(session, untitled, "new USD document")
    inspected = wait_for(session, lambda: inspect_document(session, doc), "new USD stage readiness")
    if not inspected["prim"]["exists"] or inspected["layers"]["root"]["id"] != "@root@":
        raise RuntimeError(f"unexpected document edit target: {inspected}")
    for name, kind in ((marker, "Xform"), ("TestLight", "DistantLight")):
        execute(session, "ApplyUsdOp", doc_id=doc, op={"AddPrim": {
            "edit_target": "@root@", "parent_path": "/World", "name": name,
            "type_name": kind, "reference": None, "reference_prim_path": None,
        }})
    if runner:
        execute(session, "ApplyUsdOp", doc_id=doc, op={"AddPrim": {
            "edit_target": "@root@", "parent_path": "/World", "name": "SceneLoadingGate",
            "type_name": "Scope", "reference": None, "reference_prim_path": None,
        }})
        execute(session, "ApplyUsdOp", doc_id=doc, op={"SetApiSchemas": {
            "edit_target": "@root@", "path": "/World/SceneLoadingGate", "schemas": ["LunCoProgramAPI"],
        }})
        for name, kind, value in (("info:implementationSource", "token", '"sourceAsset"'),
                                  ("info:sourceAsset", "asset", "@lunco://scenarios/tests/scene_loading_settings.rhai@"),
                                  ("lunco:program:realtimeSafe", "bool", "false")):
            execute(session, "ApplyUsdOp", doc_id=doc, op={"SetAttribute": {
                "edit_target": "@root@", "path": "/World/SceneLoadingGate",
                "name": name, "type_name": kind, "value": value,
            }})
    path = root / filename
    execute(session, "SaveAsDocument", doc_id=doc, path=str(path))
    wait_for(session, path.is_file, "USD save")
    execute(session, "InspectUsdDocument", doc_id=doc, path=f"/World/{marker}")
    execute(session, "CloseDocument", doc_id=doc)
    wait_for(session, lambda: not execute(session, "ListOpenDocuments")["open_documents"], "closed fixture document")


def reopen(session, root, expected, source, marker):
    previous = twin_id(session)
    execute(session, "OpenTwin", path=str(root))
    current = wait_selection(session, previous, expected, source, marker)
    verdict(session, "verify_scene_loading", expected, source, None, marker)
    return current


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    args = parser.parse_args()
    area = ROOT / "target" / ("scene-loading-settings-" + uuid.uuid4().hex)
    root = area / "project"
    root.mkdir(parents=True)
    (root / "tools").mkdir()
    (root / "tools/scene_loading_gate.rhai").write_text("fn scene_loading_gate_probe() { 42 }\n")
    plain = area / "plain-folder"
    (plain / "tools").mkdir(parents=True)
    (plain / "tools/scene_loading_gate.rhai").write_text("fn scene_loading_gate_probe() { 42 }\n")
    (plain / ".lunco").mkdir()
    (plain / ".lunco/settings.json").write_text('{"scene":{"load_default":"invalid"}}')
    (plain / "sample.mo").write_text("model Sample Real x; equation x = 1; end Sample;\n")
    (plain / "sample.rhai").write_text("fn sample() { 42 }\n")
    (plain / "child").mkdir()
    (plain / "child/twin.toml").write_text("not valid toml [")
    config = area / "config"
    config.mkdir()
    os.environ["LUNCOSIM_CONFIG"] = str(config)
    os.environ["LUNCOSIM_ISOLATED_RUN"] = "1"
    os.environ.pop("LUNCOSIM_EPHEMERAL_SETTINGS", None)
    print(f"Evidence: {area}", flush=True)

    with ProductionSession(args.port, windowed=True, log_path=area / "base.log") as session:
        process = Path("/proc") / str(session.process.pid)
        if process.exists() and ((process / "cwd").resolve() != ROOT or (process / "exe").resolve() != BINARY.resolve()):
            raise RuntimeError("owned session does not match this checkout")
        print(f"Owned PID={session.process.pid}, port={args.port}", flush=True)
        author_scene(session, root, "default.usda", "DefaultMarker")
        author_scene(session, root, "explicit.usda", "ExplicitMarker")
        author_scene(session, plain, "plain.usda", "ExplicitMarker")
        execute(session, "CreateTwin", path=str(root), name="Scene settings gate", default_scene="default.usda")
        current = wait_selection(session, None, True, "global", "default")
        verdict(session, "verify_scene_loading", True, "global", None, "default")

        output = execute(session, "RunRhai", code='if !set_setting("SceneLoadingSettings.load_default", false) { throw "global setting failed"; }')
        if output.get("error"): raise RuntimeError(output)
        current = reopen(session, root, False, "global", "empty")
        execute(session, "SetProjectSceneLoading", twin=current, value=True)
        wait_for(session, lambda: execute(session, "GetSceneLoadingSettings").get("project") is True, "project save")
        current = reopen(session, root, True, "project", "default")
        execute(session, "SetTwinSetting", key="scene.load_default", value=False)
        current = reopen(session, root, False, "twin", "empty")
        rejected = rejection(session, "SetTwinSetting", key="scene.load_default", value="invalid")
        if "must be a boolean" not in rejected: raise RuntimeError(f"invalid Twin setting was accepted: {rejected}")
        project_file = root / ".lunco/settings.json"
        saved_project = project_file.read_bytes()
        malformed = b'{"scene":{"load_default":"invalid"}}'
        project_file.write_bytes(malformed)
        rejected = rejection(session, "SetProjectSceneLoading", twin=current, value=False)
        if "could not deserialize" not in rejected or project_file.read_bytes() != malformed:
            raise RuntimeError(f"invalid project setting write did not fail safely: {rejected}")
        verdict(session, "verify_project_write_failure")
        project_file.write_bytes(saved_project)
        execute(session, "SetProjectSceneLoading", twin=current, value=None)
        wait_for(session, lambda: execute(session, "GetSceneLoadingSettings").get("project") is None, "project reset")
        current = reopen(session, root, False, "twin", "empty")
        execute(session, "SetProjectSceneLoading", twin=current, value=True)
        wait_for(session, lambda: execute(session, "GetSceneLoadingSettings").get("project") is True, "project re-save")

        # An invalid local settings document fails visibly without retiring the active Twin.
        bad = area / "invalid"
        (bad / ".lunco").mkdir(parents=True)
        (bad / "twin.toml").write_text('name = "invalid project"\nversion = "0.1.0"\n')
        (bad / ".lunco/settings.json").write_text('{"scene":{"load_default":"invalid"}}')
        execute(session, "OpenTwin", path=str(bad))
        wait_for(session, lambda: "invalid" in session.log_path.read_text(errors="replace")
                 and "could not deserialize persisted section" in session.log_path.read_text(errors="replace"), "invalid settings diagnostic")
        verdict(session, "verify_retained_twin", current)

    # A process override outranks both project and Twin settings.
    with ProductionSession(args.port, windowed=True, extra_args=("--load-default-scene",), log_path=area / "cli-load.log") as session:
        execute(session, "OpenTwin", path=str(root))
        wait_selection(session, None, True, "cli", "default")
        verdict(session, "verify_scene_loading", True, "cli", None, "default")

    # An explicit scene is loaded even while automatic loading is disabled.
    with ProductionSession(args.port, windowed=True, extra_args=("--no-load-default-scene", "--scene", str(root / "explicit.usda")), log_path=area / "explicit.log") as session:
        wait_selection(session, None, False, "cli", "explicit")
        verdict(session, "verify_scene_loading", False, "cli", "explicit.usda", "explicit")
        execute(session, "SetTwinSetting", key="ui.camera_status", value=False)
        manifest = tomllib.loads((root / "twin.toml").read_text())
        if manifest["usd"]["default_scene"] != "default.usda":
            raise RuntimeError(f"explicit scene changed the authored default: {manifest}")

    with ProductionSession(args.port, windowed=True, extra_args=("--no-load-default-scene",), log_path=area / "cli-empty.log") as session:
        execute(session, "OpenTwin", path=str(root))
        wait_selection(session, None, False, "cli", "empty")
        verdict(session, "verify_scene_loading", False, "cli", None, "empty")
        execute(session, "ActivatePerspective", id="editor")
        execute(session, "OpenFile", path=str(root / "default.usda"))
        def document(path=None, untitled=False):
            return next((doc["doc_id"] for doc in execute(session, "ListOpenDocuments")["open_documents"]
                         if doc["kind"] == "usd" and
                         ((untitled and doc["origin"].get("kind") == "untitled") or
                          (path is not None and doc["origin"].get("path") == str(path)))), None)
        def is_open(doc_id):
            return any(doc["doc_id"] == doc_id for doc in execute(session, "ListOpenDocuments")["open_documents"])
        def preview_ready(doc):
            inspection = inspect_document(session, doc)
            if inspection is None:
                return False
            generation = inspection["generation"]
            return any(preview["doc_id"] == doc and preview["projection_ready"] and
                       preview["projected_generation"] == generation
                       for preview in execute(session, "InspectUsdViewport")["previews"])
        doc = wait_for(session, lambda: document(root / "default.usda"), "existing Editor document")
        wait_for(session, lambda: preview_ready(doc), "existing Editor preview")
        wait_for(session, lambda: (inspection := inspect_document(session, doc, "/World/DefaultMarker"))
                 and inspection["prim"]["exists"], "existing USD contents")
        verdict(session, "verify_empty_scene_editor", doc, "/World/DefaultMarker")
        if prim_present(session, "/World/DefaultMarker"):
            raise RuntimeError("opening an Editor preview mounted the Twin's default scene")
        session.capture_screenshot(area / "empty-scene-editor.png")
        execute(session, "CloseDocument", doc_id=doc)
        wait_for(session, lambda: not is_open(doc), "closed Editor document")

        execute(session, "NewDocument", kind="usd")
        doc = wait_for(session, lambda: document(untitled=True), "new Editor document")
        wait_for(session, lambda: inspect_document(session, doc), "new Editor stage readiness")
        execute(session, "ApplyUsdOp", doc_id=doc, op={"AddPrim": {
            "edit_target": "@root@", "parent_path": "/World", "name": "EditorMarker",
            "type_name": "Cube", "reference": None, "reference_prim_path": None,
        }})
        created = root / "editor-created.usda"
        execute(session, "SaveAsDocument", doc_id=doc, path=str(created))
        wait_for(session, created.is_file, "saved Editor document")
        wait_for(session, lambda: preview_ready(doc), "new Editor preview")
        wait_for(session, lambda: (inspection := inspect_document(session, doc, "/World/EditorMarker"))
                 and inspection["prim"]["exists"], "new USD contents")
        verdict(session, "verify_empty_scene_editor", doc, "/World/EditorMarker")
        execute(session, "CloseDocument", doc_id=doc)
        wait_for(session, lambda: not is_open(doc), "closed saved Editor document")
        execute(session, "OpenFile", path=str(created))
        doc = wait_for(session, lambda: document(created), "reopened Editor document")
        wait_for(session, lambda: preview_ready(doc), "reopened Editor preview")
        wait_for(session, lambda: (inspection := inspect_document(session, doc, "/World/EditorMarker"))
                 and inspection["prim"]["exists"], "saved USD contents")
        verdict(session, "verify_empty_scene_editor", doc, "/World/EditorMarker")
        # Allow the submitted preview image to reach the desktop compositor.
        time.sleep(3)
        session.capture_screenshot(area / "empty-scene-editor-created.png")
        if prim_present(session, "/World/EditorMarker"):
            raise RuntimeError("opening an Editor document mounted it as the live scene")
        execute(session, "CloseDocument", doc_id=doc)
        if tomllib.loads((root / "twin.toml").read_text())["usd"]["default_scene"] != "default.usda":
            raise RuntimeError("Editor document operations changed the authored default scene")

    with ProductionSession(args.port, windowed=True, extra_args=("--scene", str(plain / "plain.usda")), log_path=area / "plain.log") as session:
        scope = wait_selection(session, None, False, "global", "explicit", require_tool=False)
        verdict(session, "verify_ephemeral_workspace")
        if "no authored Twin" not in rejection(session, "SetProjectSceneLoading", twin=scope, value=True):
            raise RuntimeError("temporary workspace accepted a project setting")
        if execute(session, "ReadActiveTwinContract")["open"] or (plain / "twin.toml").exists():
            raise RuntimeError("opening a plain-folder scene fabricated a Twin manifest")
        execute(session, "OpenFile", path=str(plain / "sample.mo"))
        wait_for(session, lambda: any("sample.mo" in str(doc) for doc in
                 execute(session, "ListOpenDocuments")["open_documents"]), "open sample.mo")
        execute(session, "OpenFile", path=str(plain / "sample.rhai"))
        wait_for(session, lambda: any("sample.rhai" in tab["title"] for tab in
                 execute(session, "InspectWorkbench")["docked_instance_tabs"]), "open sample.rhai")

    runner_scene = ROOT / "assets/scenes/tests/scene_loading_settings/gate.usda"
    runner = subprocess.run([str(BINARY), "test", "--scene", str(runner_scene),
                             "--verdict-channel", "SCENE_RUNNER_OVERRIDE", "--max-ticks", "1800"],
                            cwd=ROOT, capture_output=True, text=True, timeout=180)
    (area / "scene-runner.log").write_text(runner.stdout + "\n" + runner.stderr)
    runner_output = runner.stdout + "\n" + runner.stderr
    if runner.returncode != 0 or "luncosim test PASS" not in runner_output or "channel=SCENE_RUNNER_OVERRIDE" not in runner_output:
        raise RuntimeError(f"authored scene runner failed ({runner.returncode}); log={area / 'scene-runner.log'}")

    print("PASS: layered settings, invalid project, CLI precedence, explicit scene and manifest preservation", flush=True)


if __name__ == "__main__":
    main()
