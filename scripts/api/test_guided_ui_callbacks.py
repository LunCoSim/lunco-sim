#!/usr/bin/env python3
"""Exercise popup callback replacement through the production window/input path."""

from __future__ import annotations

import argparse
import json
import os
import re
from pathlib import Path
import tempfile
import time

from runtime import BINARY, ROOT, ProductionSession, wait_for_ready
from run_editor_scene_test import wait_for_scene_time_selection


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4381)
    parser.add_argument("--artifacts", type=Path, default=ROOT / "target/guided-ui-callbacks")
    args = parser.parse_args()
    artifacts = args.artifacts.resolve()
    artifacts.mkdir(parents=True, exist_ok=True)
    source = (ROOT / "assets/scripting/tests/hud_binding_probe.rhai").read_text()

    # The standard library has a native asset watcher. Twin mounts deliberately
    # use their own reader, so install a temporary application-source fixture.
    with tempfile.TemporaryDirectory(prefix="guided-ui-", dir=ROOT / "assets") as library, \
            tempfile.TemporaryDirectory(prefix="guided-ui-") as scratch:
        library = Path(library)
        (library / "tools").mkdir()
        (library / "tools/hud_menu_binding_probe.rhai").write_text(
            "fn on_menu_action(args) { hud_binding_probe::on_menu_action(args) }\n")
        menu_file = library / "callback-menu.json"
        menu_file.write_text(json.dumps({"label": "Initial"}))
        scratch = Path(scratch)
        os.environ["LUNCOSIM_CONFIG"] = str(scratch / "config")
        twins = [scratch / "first", scratch / "second"]
        for index, twin in enumerate(twins):
            twin.mkdir()
            (twin / "twin.toml").write_text(f'name = "Callback fixture {index}"\nversion = "0.1.0"\n')
        with ProductionSession(args.port, windowed=True,
                extra_args=("--no-throttle", "--no-vsync"),
                log_path=artifacts / "runtime.log") as session:
            assert session.process is not None
            process = Path(f"/proc/{session.process.pid}")
            if process.is_dir():
                assert (process / "cwd").resolve() == ROOT.resolve()
                assert (process / "exe").resolve() == BINARY.resolve()
            print(f"Owned runtime PID {session.process.pid}, port {args.port}, binary {BINARY}", flush=True)

            def command(command_name: str, **params):
                response = session.post({"type": "ExecuteCommand", "command": command_name, "params": params})
                if response.get("error"):
                    raise RuntimeError(response)
                return response.get("data", {})

            def hook(name: str, label: str):
                return command("RunRhaiToolHook", tool="hud_binding_probe", hook=name,
                               args={"String": label}, owner_twin_id=owner)

            def verdict(name: str, *values):
                arguments = ", ".join(json.dumps(value) for value in values)
                try:
                    result = command("RunRhai", code=f'if !hud_binding_probe::{name}({arguments}) {{ throw "callback verdict failed"; }}')
                except RuntimeError:
                    session.capture_screenshot(artifacts / "failed.png")
                    raise
                if "TESTS_OK" not in result.get("stdout", ""):
                    raise RuntimeError(f"Missing authored verdict: {result}")
                print(result["stdout"].strip(), flush=True)

            def button(state: str, x=540.0, y=217.0):
                command("InjectWindowInput", event={"PointerButton": {
                    "button": "Primary", "state": state, "x": x, "y": y}})

            def click():
                button("Pressed")
                time.sleep(.03)
                button("Released")

            def settle_input():
                session.capture_screenshot(artifacts / "input-frame.png")
                hook("input_fence", "")

            def open_twin(twin: Path) -> int:
                command("OpenTwin", path=str(twin))
                deadline = time.monotonic() + 30
                while True:
                    mounted = command("ListTwin")
                    if str(twin) in json.dumps(mounted):
                        break
                    if time.monotonic() >= deadline:
                        raise RuntimeError(f"Twin did not mount: {mounted}")
                    time.sleep(.05)
                wait_for_ready(args.port)
                return command("RegisterToolLibrary", name="hud_binding_probe", source=source)["active_twin"]

            owner = open_twin(twins[0])
            test_prelude = "\n".join(p.read_text() for p in sorted((ROOT / "assets/scripting/tests/lib").glob("*.rhai")))
            scope_gate = command("RunRhai", code=test_prelude + "\n" + (ROOT / "assets/scripting/tests/test_guided_hud.rhai").read_text())
            if "TESTS_OK 23" not in scope_gate.get("stdout", ""):
                raise RuntimeError(f"guided scope verdict failed: {scope_gate}")
            print(scope_gate["stdout"].strip(), flush=True)
            command("ActivatePerspective", id="sandbox_view")
            hook("configure", "B")
            for perspective in ["rover_build", "editor"]:
                command("ActivatePerspective", id=perspective)
                session.capture_screenshot(artifacts / f"{perspective}.png")
            command("ActivatePerspective", id="sandbox_view")
            hook("publish_menu", "A")
            session.capture_screenshot(artifacts / "before-replacement.png")
            started = time.monotonic()
            button("Pressed")
            time.sleep(.03)
            hook("publish_menu", "B")
            time.sleep(.03)
            button("Released")
            if time.monotonic() - started >= .8:
                raise RuntimeError("Gesture exceeded egui click duration; replacement was not tested")
            settle_input()
            verdict("verify_state", "B", owner, False)
            session.capture_screenshot(artifacts / "after-replacement.png")
            click()
            settle_input()
            verdict("verify_state", "B", owner, True)

            # Dismissal retires the old popup; a fresh popup remains usable.
            hook("configure", "C")
            hook("publish_menu", "C")
            session.capture_screenshot(artifacts / "before-dismissal.png")
            button("Pressed", 800.0, 150.0)
            time.sleep(.03)
            button("Released", 800.0, 150.0)
            click()
            settle_input()
            verdict("verify_state", "C", owner, False)

            hook("publish_menu", "C")
            session.capture_screenshot(artifacts / "before-reset.png")
            command("ClearScene")
            wait_for_ready(args.port)
            session.capture_screenshot(artifacts / "after-reset.png")
            click()
            settle_input()
            verdict("verify_retired")

            # A watched authored JSON menu uses the real asset-policy host.
            command("RunRhai", code='if !bind_policy("application.asset.lifecycle", "menus", hud_binding_probe::menu_policy_source()).ok { throw "menu policy rejected"; }')

            def publish_workbench(label):
                offset = (artifacts / "runtime.log").stat().st_size
                menu_file.write_text(json.dumps({"label": label}))
                deadline = time.monotonic() + 10
                while f"CALLBACK_MENU_PUBLISHED {label}" not in (artifacts / "runtime.log").read_text(errors="replace")[offset:]:
                    if time.monotonic() >= deadline:
                        raise RuntimeError("watched menu contribution did not publish")
                    time.sleep(.01)
                session.capture_screenshot(artifacts / f"menu-{label}.png")
                hook("input_fence", "")

            def click_at(x, y):
                button("Pressed", x, y)
                time.sleep(.03)
                button("Released", x, y)
                settle_input()

            hook("configure", "MenuB")
            publish_workbench("MenuA")
            # Coordinates are fixed by this isolated empty-workspace fixture.
            # Keep the replacement gesture inside egui's click-duration limit.
            click_at(290.0, 18.0)
            session.capture_screenshot(artifacts / "workbench-open.png")
            started = time.monotonic()
            button("Pressed", 300.0, 46.0)
            publish_workbench("MenuB")
            button("Released", 300.0, 46.0)
            if time.monotonic() - started >= .8:
                raise RuntimeError("Workbench gesture exceeded egui click duration")
            settle_input()
            verdict("verify_state", "MenuB", owner, False)
            click_at(300.0, 46.0)
            verdict("verify_state", "MenuB", owner, True)

            # Installation survives reset, but a held gesture belongs to its
            # original Twin/scene. A fresh click then uses the new scope.
            hook("configure", "MenuB")
            click_at(290.0, 18.0)
            session.capture_screenshot(artifacts / "menu-before-reset.png")
            started = time.monotonic()
            button("Pressed", 300.0, 46.0)
            time.sleep(.03)
            command("ClearScene")
            wait_for_ready(args.port)
            hook("configure", "MenuB")
            session.capture_screenshot(artifacts / "menu-after-reset.png")
            button("Released", 300.0, 46.0)
            if time.monotonic() - started >= .8:
                raise RuntimeError("Reset gesture exceeded egui click duration")
            settle_input()
            verdict("verify_state", "MenuB", owner, False)
            click_at(300.0, 46.0)
            verdict("verify_state", "MenuB", owner, True)

            # Stop-phase callers go through both deferred queues. Inspect their
            # completed verdicts after FIFO fences, not merely accepted commands.
            command("LoadScene", path="lunco://scenes/tests/twin_session_retirement/replacement/replacement.usda")
            wait_for_scene_time_selection(session, artifacts / "runtime.log", 30)
            command("QueryUsdPrim", path="/World")
            wait_for_ready(args.port)
            command("OpenUsdSourceDocument",
                source="lunco://scenes/tests/twin_session_retirement/replacement/replacement.usda")
            fixture = (ROOT / "assets/scenes/tests/twin_session_retirement/replacement/replacement.usda").resolve()
            deadline = time.monotonic() + 15
            while True:
                documents = [entry for entry in command("ListOpenDocuments")["open_documents"]
                    if entry["origin"].get("kind") == "file"
                    and Path(entry["origin"].get("path", "")).resolve() == fixture]
                if len(documents) == 1:
                    break
                if len(documents) > 1 or time.monotonic() >= deadline:
                    raise RuntimeError(f"Expected the exact callback source document: {documents}")
                time.sleep(.05)
            verdict("verify_pointer_subscription", documents[0]["doc_id"])
            command("RunScenarioAsset", source_asset="lunco://scenarios/tests/guided_hud_teardown.rhai", owner_twin_id=owner)
            command("SetTimeTransport", playing=True)
            wait_for_ready(args.port)
            deadline = time.monotonic() + 15
            while True:
                cards = command("InspectGuidedHud")["cards"]
                teaching = [c for c in cards if c["kind"] == "script" and c["hint"] == "Teaching lifecycle probe"]
                if teaching:
                    break
                if time.monotonic() >= deadline:
                    diagnostics = command("RunRhai", code='for entity in list_entities() { let status = query("ScriptInspect", #{target: entity.id}); if status.scripted == true { print(status); } }')
                    session.capture_screenshot(artifacts / "teardown-failed.png")
                    raise RuntimeError(f"teardown scenario did not start: {diagnostics}; cards={cards}")
                time.sleep(.05)
            command("ActivatePerspective", id="rover_build")
            session.capture_screenshot(artifacts / "teaching-in-builder.png")
            verdict("verify_teaching", teaching[0]["author"], True)
            command("ActivatePerspective", id="sandbox_view")
            offset = (artifacts / "runtime.log").stat().st_size
            command("StopScenario", target=teaching[0]["author"])
            settle_input()
            output = (artifacts / "runtime.log").read_text(errors="replace")[offset:]
            requests = re.search(r"HUD_STOP_REQUESTS (\d+) (\d+)", output)
            if requests is None or "HUD teardown: PASS" not in output:
                raise RuntimeError(f"missing teardown verdict: {output}")
            verdict("verify_stopped_requests", [int(value) for value in requests.groups()])

            hook("configure", "D")
            hook("publish_menu", "D")
            session.capture_screenshot(artifacts / "before-twin-replacement.png")
            owner = open_twin(twins[1])
            session.capture_screenshot(artifacts / "after-twin-replacement.png")
            click()
            settle_input()
            verdict("verify_retired")
            hook("configure", "E")
            hook("publish_menu", "E")
            session.capture_screenshot(artifacts / "replacement-twin.png")
            click()
            settle_input()
            verdict("verify_state", "E", owner, True)
    print("Guided UI callbacks passed; owned session exited and port released.")


if __name__ == "__main__":
    main()
