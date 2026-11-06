#!/usr/bin/env python3
"""Click the empty-Twin scene choices in an owned production session."""

import argparse
import json
import os
from pathlib import Path
import time
import uuid

from runtime import BINARY, ROOT, ProductionSession
from test_scene_loading_settings import author_scene, execute, prim_present, verdict, wait_for, wait_selection


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    args = parser.parse_args()
    area = ROOT / "target" / ("scene-loading-ui-" + uuid.uuid4().hex)
    root = area / "project"
    (root / "scenes").mkdir(parents=True)
    (root / "tools").mkdir()
    (root / ".lunco").mkdir()
    (root / ".lunco/settings.json").write_text('{"scene":{"load_default":false}}')
    (root / "tools/scene_loading_gate.rhai").write_text("fn scene_loading_gate_probe() { 42 }\n")
    os.environ["LUNCOSIM_CONFIG"] = str(area / "config")
    os.environ["LUNCOSIM_ISOLATED_RUN"] = "1"
    os.environ.pop("LUNCOSIM_EPHEMERAL_SETTINGS", None)
    print(f"Evidence: {area}", flush=True)

    with ProductionSession(args.port, windowed=True,
                           extra_args=("--no-throttle", "--window-pos", "1280x720+0+0"),
                           log_path=area / "runtime.log") as session:
        proc = Path("/proc") / str(session.process.pid)
        if proc.exists() and ((proc / "cwd").resolve() != ROOT or (proc / "exe").resolve() != BINARY.resolve()):
            raise RuntimeError("production session does not belong to this checkout")
        print(f"Owned PID={session.process.pid}, port={args.port}", flush=True)
        author_scene(session, root, "default.usda", "DefaultMarker")
        author_scene(session, root, "scenes/other.usda", "ExplicitMarker")
        execute(session, "CreateTwin", path=str(root), name="Scene settings gate", default_scene="default.usda")
        current = wait_selection(session, None, False, "project", "empty")
        session.capture_screenshot(area / "empty-twin.png")

        def click(x, y):
            execute(session, "InjectWindowInput", event={"PointerMove": {"x": x, "y": y}})
            time.sleep(.1)
            for state in ("Pressed", "Released"):
                execute(session, "InjectWindowInput", event={"PointerButton": {
                    "button": "Primary", "state": state, "x": x, "y": y,
                }})
                time.sleep(.1)

        def ready_scene(path):
            wait_for(session, lambda: prim_present(session, path), "selected scene")
            wait_for(session, lambda: (status := execute(session, "GetReadiness"))["ready"]
                     and not status["world_hold"] and not status["faulted"], "scene readiness")
            time.sleep(.5)

        def move(x, y):
            execute(session, "InjectWindowInput", event={"PointerMove": {"x": x, "y": y}})
            time.sleep(1)

        # Global and project controls use ordinary Settings submenus.
        click(355.0, 17.0)
        move(380.0, 118.0)
        move(540.0, 118.0)
        click(525.0, 118.0)
        wait_for(session, lambda: execute(session, "GetSceneLoadingSettings")["global"] is False, "global menu setting")
        verdict(session, "verify_settings_menu", False, False, "project")
        execute(session, "InjectWindowInput", event={"Key": {"key": "Escape", "state": "Pressed", "repeat": False, "text": None}})
        execute(session, "InjectWindowInput", event={"Key": {"key": "Escape", "state": "Released", "repeat": False, "text": None}})
        click(355.0, 17.0)
        move(380.0, 139.0)
        move(650.0, 160.0)
        move(900.0, 160.0)
        click(900.0, 160.0)
        wait_for(session, lambda: execute(session, "GetSceneLoadingSettings")["project"] is None, "project menu inheritance")
        verdict(session, "verify_settings_menu", None, False, "global")
        if "load_default" in json.loads((root / ".lunco/settings.json").read_text()).get("scene", {}):
            raise RuntimeError("project inheritance did not remove the local override")
        click(355.0, 17.0)
        move(380.0, 139.0)
        move(650.0, 160.0)
        move(1040.0, 202.0)
        click(1040.0, 202.0)
        wait_for(session, lambda: execute(session, "GetSceneLoadingSettings")["project"] is False, "project menu override")
        verdict(session, "verify_settings_menu", False, False, "project")
        if json.loads((root / ".lunco/settings.json").read_text())["scene"]["load_default"] is not False:
            raise RuntimeError("project menu did not persist the local override")
        if any(panel["id"] == "lunco.workbench.project_settings" for panel in execute(session, "InspectWorkbench")["panels"]):
            raise RuntimeError("project settings unexpectedly registered a custom panel")

        click(640.0, 359.0)
        ready_scene("/World/DefaultMarker")
        verdict(session, "verify_scene_loading", False, "project", None, "default")
        execute(session, "OpenTwin", path=str(root))
        current = wait_selection(session, current, False, "project", "empty")
        click(640.0, 415.0)
        ready_scene("/World/ExplicitMarker")
        verdict(session, "verify_scene_loading", False, "project", None, "explicit")
        empty = area / "empty-folder"
        empty.mkdir()
        execute(session, "OpenFolder", path=str(empty))
        wait_for(session, lambda: (scope := execute(session, "ReadActiveWorkspace"))
                 and scope.get("kind") == "ephemeral" and scope.get("root") == str(empty), "empty folder")
        time.sleep(1)
        session.capture_screenshot(area / "empty-folder.png")
        print("PASS: scoped settings menus, default scene, listed scene, and empty folder", flush=True)


if __name__ == "__main__":
    main()
