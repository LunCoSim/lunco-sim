#!/usr/bin/env python3
"""Drive Twin replacement in a windowed production host; Rhai owns verdicts."""

from __future__ import annotations

import json
import os
import subprocess
import time
import tomllib
from contextlib import ExitStack
from pathlib import Path

from runtime import BINARY, ProductionSession, ROOT, get_json, wait_for_ready

PORT = int(os.environ.get("LUNCOSIM_API_PORT", "4187"))
SOURCE = ROOT / "assets/scenarios/tests/twin_session_retirement.rhai"
FIXTURE = ROOT / "assets/scenes/tests/twin_session_retirement"
TWIN_A = Path(os.environ.get("LUNCOSIM_TEST_TWIN_A", str(FIXTURE))).resolve()
TWIN_B = Path(os.environ.get("LUNCOSIM_TEST_TWIN_B", str(FIXTURE / "replacement"))).resolve()


def execute(session: ProductionSession, command: str, **params) -> dict:
    response = session.post({"type": "ExecuteCommand", "command": command, "params": params})
    if response.get("error"):
        raise RuntimeError(f"{command}: {response}")
    return response.get("data", {})


def documents(session: ProductionSession) -> list[dict]:
    return execute(session, "ListOpenDocuments")["open_documents"]


def catalog_paths(session: ProductionSession) -> list[str]:
    return wait_for(session, lambda: [entry["path"] for entry in execute(session, "ListUsdAssetMetadata")["entries"]
                                     if entry["path"].startswith("twin://")], "Twin catalog publication")


def live_prim(session: ProductionSession, path: str, **params) -> dict | None:
    try:
        return execute(session, "QueryUsdPrim", path=path, **params)
    except RuntimeError as error:
        if ("HTTP 404" in str(error) or "no USD stage loaded" in str(error)
                or "exactly one mounted live stage is required" in str(error)):
            return None
        raise


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


def open_twin(session: ProductionSession, root, previous_ids=(), command="OpenTwin", file_uri=False) -> list[int]:
    telemetry = seed_telemetry(session) if previous_ids else None
    path = root
    if command == "OpenFile":
        manifest = tomllib.loads((root / "twin.toml").read_text(encoding="utf-8"))
        path = root / manifest["usd"]["default_scene"]
    print(f"Switch route: {command}, path={path}", flush=True)
    retired_authority = execute(session, "RunRhai", code="print(twin_name());")["stdout"].strip()
    execute(session, command, path=path.as_uri() if file_uri else str(path))
    ids = wait_for_twin(session, root, previous_ids)
    authority = execute(session, "RunRhai", code="print(twin_name());")["stdout"].strip()
    def source_terminal():
        analysis = execute(session, "AnalyzeSysml", path="twin://" + authority, tables=[])
        return not any("preparing asynchronously" in error for error in analysis.get("errors", []))
    wait_for(session, source_terminal, "replacement SysML source preparation")
    verdict("verify_twin_source_scope", retired_authority)
    if telemetry:
        telemetry_retirement_verdict(session, telemetry)
    return ids


def seed_telemetry(session: ProductionSession) -> tuple[list[str], list[str]]:
    prim = live_prim(session, "/World", topology=True)
    if not prim or not prim.get("spawned"):
        raise RuntimeError("telemetry probe needs the current mounted /World")
    # The plain Xform has session-local identity; the topology owner supplies
    # its exact native Entity. Solver channels separately exercise API owners.
    execute(session, "ControlTelemetry", channel="lifecycle_retirement_probe",
            entity=prim["topology"]["binding"]["entity"], reflect="Transform.translation.x",
            unit="m", rate_hz=60.0)
    execute(session, "SetTimeTransport", playing=True)
    def sampled():
        channels = execute(session, "ListTelemetryChannels")["channels"]
        return channels if any(channel["name"] == "lifecycle_retirement_probe"
                               and channel["samples"] > 0 for channel in channels) else None
    channels = wait_for(session, sampled, "outgoing telemetry samples")
    return ([channel["key"] for channel in channels if not channel["name"].startswith("engine.")],
            [channel["key"] for channel in channels if channel["name"].startswith("engine.")])


def paused_modelica_probe(session: ProductionSession, name: str) -> tuple[int, str]:
    doc = execute(session, "CreateScratchModelicaDocument", name=name,
                  source=f"model {name} Real x(start=37, fixed=true); equation der(x)=0; end {name};")["doc_id"]
    wait_for(session, lambda: execute(session, "CompileStatus", doc_id=doc).get("ast_parsed"),
             "paused probe parse")
    execute(session, "CompileModel", doc_id=doc, **{"class": name, "resume_after_compile": True})
    def recorded():
        return [channel for channel in execute(session, "ListTelemetryChannels")["channels"]
                if channel.get("model_class") == name and channel["samples"] > 0]
    channels = wait_for(session, recorded, "paused probe samples")
    execute(session, "PauseActiveModel", doc_id=doc)
    wait_for(session, lambda: execute(session, "CompileStatus", doc_id=doc).get("paused"),
             "paused probe solver")
    return doc, channels[0]["key"]


def telemetry_retirement_verdict(session: ProductionSession, telemetry: tuple[list[str], list[str]]) -> None:
    # Projection readiness can precede the first fixed sample and its delivery.
    # Rhai owns both the publication prerequisite and the final clock assertion.
    code = SOURCE.read_text(encoding="utf-8") + "\nprint(application_telemetry_current(" + json.dumps(telemetry[1]) + "));"
    wait_for(session, lambda: execute(session, "RunRhai", code=code)["stdout"].strip() == "true",
             "replacement application telemetry publication")
    verdict("verify_telemetry_retirement", *telemetry)


def experiment_playback_probe(session: ProductionSession, doc: int, name: str, cleanup: ExitStack) -> tuple[str, list[str]]:
    run = execute(session, "RunExperiment", doc_id=doc,
                  **{"class": name, "t_end": 0.1, "n_intervals": 10})["experiment_id"]
    # Remove only this run's generated artifact, after the owned app exits.
    cleanup.callback((TWIN_B / "results" / f"{run}.json").unlink, missing_ok=True)
    wait_for(session, lambda: execute(session, "RunStatus", experiment_id=run).get("state") == "done",
             "experiment playback completion")
    keys = wait_for(session, lambda: [channel["key"] for channel in execute(session, "ListTelemetryChannels")["channels"]
                                     if channel["name"] == "playback_probe_value" and channel["samples"] > 0],
                    "experiment playback publication")
    return run, keys


def wait_for_twin(session: ProductionSession, root, previous_ids=()) -> list[int]:
    def admitted():
        # The authored probe creates and closes transient scratch documents;
        # retain the scene documents whose lifetime is the Twin itself.
        docs = [doc for doc in documents(session) if doc["kind"] == "usd"]
        paths = [str(doc.get("origin", {}).get("path", "")).replace("\\", "/") for doc in docs]
        ids = [doc["doc_id"] for doc in docs]
        if any(str(root).replace("\\", "/") in path for path in paths) and not set(ids).intersection(previous_ids):
            return ids
        return None
    wait_for(session, admitted, f"fresh documents for {root.name}")
    def projected():
        ready = get_json(session.port, "/api/ready")["data"]
        if ready.get("faulted"):
            raise RuntimeError(f"replacement faulted: {ready.get('fault')}")
        if not ready.get("ready") or ready.get("world_hold"):
            return False
        scenes = [doc for doc in documents(session) if doc["kind"] == "usd"
                  and doc["runtime_context"]["kind"] == "local_twin"]
        # Scene admission is owned by the primary live binding. Editor-preview
        # stages may still prepare independently after world readiness.
        prim = live_prim(session, "/World")
        return (prim and prim.get("spawned")
                and prim.get("doc_id") in {scene["doc_id"] for scene in scenes})
    wait_for(session, projected, f"replacement scene projection for {root.name}")
    wait_for_ready(session.port)
    return [doc["doc_id"] for doc in documents(session) if doc["kind"] == "usd"]


def verdict(function: str, *args) -> None:
    invocation = function + "(" + ",".join(json.dumps(arg) for arg in args) + ")"
    code = SOURCE.read_text(encoding="utf-8") + "\n" + invocation + ";"
    result = subprocess.run(
        [str(BINARY), "rhai", "--api", str(PORT), "--stdout", "-e", code],
        cwd=ROOT, capture_output=True, text=True, timeout=30,
    )
    print(result.stdout.strip())
    if result.returncode or "TESTS_OK" not in result.stdout or "TESTS_FAIL" in result.stdout:
        raise RuntimeError(f"authored Rhai verdict failed: {result.stdout}\n{result.stderr}")


def main() -> None:
    with ExitStack() as cleanup, ProductionSession(PORT, windowed=True, log_path=ROOT / "target/twin-session-retirement.log") as session:
        process_root = Path("/proc") / str(session.process.pid)
        if process_root.is_dir():
            if (process_root / "cwd").resolve() != ROOT.resolve() or (process_root / "exe").resolve() != BINARY.resolve():
                raise RuntimeError("production process does not belong to this checkout/binary")
        print(f"Owned production session: PID={session.process.pid}, port={PORT}, checkout={ROOT}", flush=True)
        try:
            ids_a = open_twin(session, TWIN_A)
            execute(session, "ActivatePerspective", id="editor")
            scene = next(doc for doc in documents(session) if doc["kind"] == "usd")
            execute(session, "OpenFile", path=scene["origin"]["path"])
            scene_doc = scene["doc_id"]
            execute(session, "OpenUsdPreview", preview=9001, doc_id=scene_doc, edit_target="@root@")
            execute(session, "OpenUsdPreview", preview=9002, doc_id=scene_doc, edit_target="@root@")
            wait_for(session, lambda: execute(session, "InspectUsdViewport")["preview_count"] >= 2,
                     "both Twin A editor previews")
            scratch = execute(session, "CreateScratchModelicaDocument", name="TwinRetirementScratch",
                    source="model TwinRetirementScratch Real x(start=37, fixed=true); equation der(x)=0; end TwinRetirementScratch;")
            wait_for(session, lambda: any(doc["kind"] == "modelica" for doc in documents(session)), "Modelica scratch")
            wait_for(session, lambda: execute(session, "CompileStatus", doc_id=scratch["doc_id"]).get("ast_parsed"),
                     "Modelica scratch parse")
            execute(session, "CompileModel", doc_id=scratch["doc_id"],
                    **{"class": "TwinRetirementScratch", "resume_after_compile": True})
            wait_for(session, lambda: any(channel.get("model_class") == "TwinRetirementScratch"
                                         and channel["samples"] > 0
                                         for channel in execute(session, "ListTelemetryChannels")["channels"]),
                     "outgoing Modelica telemetry")
            verdict("verify_modelica_telemetry_recorded", "TwinRetirementScratch")
            execute(session, "NewDocument", kind="sysml")
            wait_for(session, lambda: any(doc["kind"] == "sysml" for doc in documents(session)), "dirty SysML draft")
            sysml_ids = [doc["doc_id"] for doc in documents(session) if doc["kind"] == "sysml"]
            modelica_ids = [doc["doc_id"] for doc in documents(session) if doc["kind"] == "modelica"]
            ids_a = [doc["doc_id"] for doc in documents(session)]
            paths_a = catalog_paths(session)
            seed_telemetry(session)
            execute(session, "FocusPanel", id="telemetry_browser")
            execute(session, "SetTelemetryBrowserView", filter="lifecycle_retirement_probe",
                    signal="lifecycle_retirement_probe")
            wait_for(session, lambda: (view := execute(session, "InspectTelemetryCatalog"))["initial_scans"] > 0
                     and not view["worker_active"] and not view["pending_channels"], "outgoing telemetry browser")
            session.capture_screenshot(ROOT / "target/twin-telemetry-before-replacement.png")
            ids_b = open_twin(session, TWIN_B, ids_a)
            execute(session, "FocusPanel", id="telemetry_browser")
            wait_for(session, lambda: (view := execute(session, "InspectTelemetryCatalog"))["initial_scans"] > 0
                     and not view["worker_active"] and not view["pending_channels"], "replacement telemetry browser")
            session.capture_screenshot(ROOT / "target/twin-telemetry-after-replacement.png")
            verdict("verify_catalog_retirement", paths_a)
            verdict("verify_editor_retirement", ids_a, ids_b)
            verdict("verify_sysml_retirement", sysml_ids)
            verdict("verify_modelica_retirement", modelica_ids)
            verdict("verify_twin_browser_retirement", ids_a, ids_b, str(TWIN_B))

            # Same-path reopening is a fresh session, even when no scene path changes.
            paths_b = catalog_paths(session)
            ids_reload = open_twin(session, TWIN_B, ids_b)
            verdict("verify_catalog_retirement", paths_b)
            verdict("verify_editor_retirement", ids_b, ids_reload)
            verdict("verify_twin_browser_retirement", ids_b, ids_reload, str(TWIN_B))

            # Invalid candidates must leave the admitted session untouched.
            preserved_telemetry, _ = seed_telemetry(session)
            session.post({"type": "ExecuteCommand", "command": "OpenTwin",
                          "params": {"path": str(ROOT / "target/no-such-twin-retirement-negative")}})
            verdict("verify_session_preserved", ids_reload)
            verdict("verify_telemetry_preserved", preserved_telemetry)

            # Repeated replacement cannot accumulate source documents or previews.
            paths_reload = catalog_paths(session)
            ids_return = open_twin(session, TWIN_A, ids_reload)
            verdict("verify_catalog_retirement", paths_reload)
            verdict("verify_editor_retirement", ids_reload, ids_return)
            verdict("verify_twin_browser_retirement", ids_reload, ids_return, str(TWIN_A))
            # Retire the probe Twin before cleanup: its authored task can still
            # create dirty scratch documents after a viewport-only ClearScene.
            paths_return = catalog_paths(session)
            ids_final = open_twin(session, TWIN_B, ids_return)
            verdict("verify_catalog_retirement", paths_return)
            verdict("verify_editor_retirement", ids_return, ids_final)
            verdict("verify_twin_browser_retirement", ids_return, ids_final, str(TWIN_B))
            # Native folder picker and native/file-URI scene-file opening are
            # separate production entry points into workspace replacement.
            # The replacement fixture is nested under A. OpenFile within an
            # admitted root is document-only, so external file routes start in B.
            for root, command, file_uri in [(TWIN_A, "OpenFolder", False),
                                           (TWIN_B, "OpenTwin", False),
                                           (TWIN_A, "OpenFile", False),
                                           (TWIN_B, "OpenFolder", False),
                                           (TWIN_A, "OpenFile", True),
                                           (TWIN_B, "OpenFolder", False)]:
                outgoing = [doc["doc_id"] for doc in documents(session)]
                outgoing_models = [doc["doc_id"] for doc in documents(session) if doc["kind"] == "modelica"]
                paths = catalog_paths(session)
                ids_final = open_twin(session, root, outgoing, command, file_uri)
                verdict("verify_editor_retirement", outgoing, ids_final)
                verdict("verify_twin_browser_retirement", outgoing, ids_final, str(root))
                verdict("verify_catalog_retirement", paths)
                if outgoing_models:
                    verdict("verify_modelica_retirement", outgoing_models)
            if os.environ.get("LUNCOSIM_TEST_RECENT_MENU") == "1":
                recents = execute(session, "ListRecentFiles")["recent_twins"]
                if [entry["path"] for entry in recents[:2]] != [str(TWIN_B), str(TWIN_A)]:
                    raise RuntimeError("Recent Twin menu does not contain the expected two roots")
                outgoing = [doc["doc_id"] for doc in documents(session)]
                paths = catalog_paths(session)
                execute(session, "ActivatePerspective", id="editor")
                def move(x, y):
                    execute(session, "InjectWindowInput", event={"PointerMove": {"x": x, "y": y}})
                    time.sleep(.03)
                def click(x, y):
                    move(x, y)
                    for state in ("Pressed", "Released"):
                        execute(session, "InjectWindowInput", event={"PointerButton": {
                            "button": "Primary", "state": state, "x": x, "y": y}})
                        time.sleep(.03)
                # Same default-scale coordinates as test_twin_loading_replacement.py.
                click(18, 14)
                move(100, 120)
                move(250, 140)
                session.capture_screenshot(ROOT / "target/twin-retirement-recent-menu.png")
                click(250, 140)
                print("Switch route: File → Open Recent Twin", flush=True)
                current = wait_for_twin(session, TWIN_A, outgoing)
                verdict("verify_editor_retirement", outgoing, current)
                verdict("verify_twin_browser_retirement", outgoing, current, str(TWIN_A))
                verdict("verify_catalog_retirement", paths)
                paths = catalog_paths(session)
                ids_final = open_twin(session, TWIN_B, current)
                verdict("verify_editor_retirement", current, ids_final)
                verdict("verify_twin_browser_retirement", current, ids_final, str(TWIN_B))
                verdict("verify_catalog_retirement", paths)
            # Scenarios and tutorials use scene transitions within the open Twin.
            # Exercise a different scene and Restart Scenario without changing
            # the workspace root or reviving the previous Twin's documents.
            print("Scene routes: LoadScene and RestartScene", flush=True)
            scene_path = "lunco://scenes/tests/twin_session_retirement/replacement/replacement.usda"
            telemetry = seed_telemetry(session)
            execute(session, "LoadScene", path=scene_path)
            def scenario_projected():
                data = live_prim(session, "/World", topology=True)
                return data if data and data.get("spawned") and data.get("doc_id") not in ids_final else False
            wait_for(session, scenario_projected, "scenario scene projection")
            wait_for_ready(session.port)
            telemetry_retirement_verdict(session, telemetry)
            before_interaction_restart = scenario_projected()["topology"]["binding"]["entity"]
            code = SOURCE.read_text(encoding="utf-8") + "\nqueue_scene_interactions_and_restart();"
            interaction_ids = json.loads(execute(session, "RunRhai", code=code)["stdout"].strip())
            wait_for(session, lambda: (scene := scenario_projected()) and scene["topology"]["binding"]["entity"]
                     != before_interaction_restart, "queued interaction scene restart")
            wait_for_ready(session.port)
            ready_code = (SOURCE.read_text(encoding="utf-8")
                          + "\nprint(interaction_results_ready(" + json.dumps(interaction_ids) + "));")
            wait_for(session, lambda: execute(session, "RunRhai", code=ready_code)["stdout"].strip() == "true",
                     "queued interaction terminal results")
            verdict("verify_scene_interaction_retirement", interaction_ids)
            verdict("verify_twin_browser_retirement", current if os.environ.get("LUNCOSIM_TEST_RECENT_MENU") == "1" else outgoing,
                    [doc["doc_id"] for doc in documents(session)], str(TWIN_B))
            before_restart = scenario_projected()
            telemetry = seed_telemetry(session)
            execute(session, "RestartScene")
            wait_for(session, lambda: (scene := scenario_projected()) and scene["topology"]["binding"]["entity"]
                     != before_restart["topology"]["binding"]["entity"],
                     "restarted scene projection")
            wait_for_ready(session.port)
            telemetry_retirement_verdict(session, telemetry)
            verdict("verify_twin_browser_retirement", current if os.environ.get("LUNCOSIM_TEST_RECENT_MENU") == "1" else outgoing,
                    [doc["doc_id"] for doc in documents(session)], str(TWIN_B))
            # Independent editor solvers survive scene reload. A paused source
            # must republish current state after history retirement without a tick.
            paused_doc, paused_key = paused_modelica_probe(session, "TwinPausedReloadProbe")
            old_entity = scenario_projected()["topology"]["binding"]["entity"]
            execute(session, "RestartScene")
            wait_for(session, lambda: (scene := scenario_projected()) and scene["topology"]["binding"]["entity"] != old_entity,
                     "paused-source scene restart")
            wait_for_ready(session.port)
            verdict("verify_paused_modelica_reload", paused_doc, paused_key, 37.0)
            survivor_doc, survivor_key = paused_modelica_probe(session, "TwinPausedReloadSurvivor")
            application_keys = [channel["key"] for channel in execute(session, "ListTelemetryChannels")["channels"]
                                if channel["name"].startswith("engine.")]
            execute(session, "CloseDocument", doc_id=paused_doc)
            wait_for(session, lambda: all(doc["doc_id"] != paused_doc for doc in documents(session)),
                     "paused probe document closure")
            verdict("verify_closed_modelica_telemetry", paused_doc, [paused_key], [survivor_key, *application_keys])
            execute(session, "CloseDocument", doc_id=survivor_doc)
            wait_for(session, lambda: all(doc["doc_id"] != survivor_doc for doc in documents(session)),
                     "surviving probe document closure")
            verdict("verify_closed_modelica_telemetry", survivor_doc, [survivor_key], application_keys)
            playback_name = "TwinPlaybackReloadProbe"
            playback_doc = execute(session, "CreateScratchModelicaDocument", name=playback_name,
                source=f"model {playback_name} Real playback_probe_value(start=37, fixed=true); equation der(playback_probe_value)=0; end {playback_name};")["doc_id"]
            wait_for(session, lambda: execute(session, "CompileStatus", doc_id=playback_doc).get("ast_parsed"),
                     "experiment playback parse")
            playback_run, playback_keys = experiment_playback_probe(session, playback_doc, playback_name, cleanup)
            old_entity = scenario_projected()["topology"]["binding"]["entity"]
            execute(session, "RestartScene")
            wait_for(session, lambda: (scene := scenario_projected()) and scene["topology"]["binding"]["entity"] != old_entity,
                     "experiment playback scene restart")
            wait_for_ready(session.port)
            verdict("verify_experiment_playback_retirement", playback_doc, playback_run, playback_keys, [], True)
            playback_run, fresh_playback_keys = experiment_playback_probe(session, playback_doc, playback_name, cleanup)
            verdict("verify_experiment_playback_retirement", playback_doc, playback_run, playback_keys, fresh_playback_keys, True)
            execute(session, "CloseDocument", doc_id=playback_doc)
            wait_for(session, lambda: all(doc["doc_id"] != playback_doc for doc in documents(session)),
                     "experiment playback document closure")
            verdict("verify_experiment_playback_retirement", playback_doc, playback_run, fresh_playback_keys, [], False)
            execute(session, "FocusPanel", id="usd_connection_canvas")
            def outgoing_diagram():
                view = execute(session, "InspectConnectionDiagram")
                return view if view.get("built") and view.get("view_id") else None
            diagram = wait_for(session, outgoing_diagram, "outgoing Connections graph")
            execute(session, "CreateConnectionView", view_id=diagram["view_id"], name="RetirementProbe", scope=diagram["scope"])
            session.capture_screenshot(ROOT / "target/editor-connections-before-reload.png")
            execute(session, "ActivatePerspective", id="sandbox_view")
            old_entity = scenario_projected()["topology"]["binding"]["entity"]
            execute(session, "RestartScene")
            wait_for(session, lambda: (scene := scenario_projected()) and scene["topology"]["binding"]["entity"] != old_entity,
                     "hidden editor scene restart")
            wait_for_ready(session.port)
            verdict("verify_hidden_editor_scene_retirement")
            execute(session, "FocusPanel", id="usd_connection_canvas")
            wait_for(session, outgoing_diagram, "reopened Connections graph")
            verdict("verify_editor_diagram_reopened", diagram["view_id"])
            session.capture_screenshot(ROOT / "target/editor-connections-after-reload.png")
            retained = [doc["doc_id"] for doc in documents(session)]
            for root, command in [(TWIN_A, "AddTwin"), (FIXTURE / "replacement", "AddFolderToWorkspace")]:
                print(f"Additive route: {command}, path={root}", flush=True)
                execute(session, command, path=str(root))
                relative = tomllib.loads((root / "twin.toml").read_text(encoding="utf-8"))["usd"]["default_scene"]
                wait_for(session, lambda: any(path.endswith("/" + relative) for path in catalog_paths(session)),
                         "added Twin catalog publication")
                verdict("verify_additive_workspace", retained, str(TWIN_B))
            paths = catalog_paths(session)
            ids_final = open_twin(session, TWIN_B, retained)
            verdict("verify_editor_retirement", retained, ids_final)
            verdict("verify_twin_browser_retirement", retained, ids_final, str(TWIN_B))
            verdict("verify_catalog_retirement", paths)
            screenshot = os.environ.get("LUNCOSIM_TEST_SCREENSHOT")
            if screenshot:
                execute(session, "ActivatePerspective", id="editor")
                session.capture_screenshot(Path(screenshot))
        finally:
            execute(session, "ClearScene")
            for doc in documents(session):
                execute(session, "CloseDocument", doc_id=doc["doc_id"])
            wait_for(session, lambda: not documents(session), "test document cleanup before Exit")
        verdict("verify_editor_retirement", ids_final, [])
        verdict("verify_scene_interaction_requires_scene")
        try:
            execute(session, "RunRhaiTool", tool="gizmo", args={"Map": {}}, scene_bound=True)
        except RuntimeError as error:
            admission_error = str(error)
        else:
            admission_error = ""
        verdict("verify_scene_interaction_transport_rejection", admission_error)
        print(f"PASS: Twin switching, same-Twin reopening, rejected replacement; owned PID={session.process.pid}, port={PORT}")
    print("API Exit completed; owned process and port released")


if __name__ == "__main__":
    main()
