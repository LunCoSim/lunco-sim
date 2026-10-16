#!/usr/bin/env python3
"""Exercise popup callback replacement through the production window/input path."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import tempfile
import time

from runtime import BINARY, ROOT, ProductionSession, wait_for_ready


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4381)
    parser.add_argument("--artifacts", type=Path, default=ROOT / "target/guided-ui-callbacks")
    args = parser.parse_args()
    artifacts = args.artifacts.resolve()
    artifacts.mkdir(parents=True, exist_ok=True)
    source = (ROOT / "assets/scripting/tests/hud_binding_probe.rhai").read_text()

    with tempfile.TemporaryDirectory(prefix="guided-ui-") as scratch:
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
            command("ActivatePerspective", id="sandbox_view")
            hook("configure", "B")
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
