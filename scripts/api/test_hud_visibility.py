#!/usr/bin/env python3
"""Native minimize, dock, hide and menu restoration for a named guidance card."""
from __future__ import annotations

import argparse
import json
import os
import tempfile
import time
from pathlib import Path

from runtime import BINARY, ROOT, ProductionSession, assert_render_log_clean, get_json, wait_for_authored_verdict
from test_classroom_onboarding import command, wait_until


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--output", type=Path, default=ROOT / "target/hud-visibility")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="hud-visibility-") as config:
        os.environ["LUNCOSIM_CONFIG"] = config
        with ProductionSession(args.port, windowed=True,
                extra_args=("--no-throttle", "--no-vsync", "--window-pos", "1280x720+0+0"),
                log_path=output / "runtime.log") as session:
            pid = session.process.pid
            assert Path(f"/proc/{pid}/cwd").resolve() == ROOT
            assert Path(f"/proc/{pid}/exe").resolve() == BINARY.resolve()
            print(f"Owned PID {pid}, API {args.port}", flush=True)
            schema = get_json(args.port, "/api/commands/schema")
            (output / "schema.json").write_text(json.dumps(schema, indent=2) + "\n")

            def inspect():
                return command(session, "InspectGuidedHud")

            def runtime_hud():
                return next((hud for hud in command(session, "InspectRuntimeHuds")["huds"]
                             if hud["id"] == "camera-status"), {})

            def card():
                return next((c for c in inspect()["cards"] if c.get("hud") == "visibility-probe"), {})

            def expect(description, **fields):
                def matches():
                    state = card()
                    return state if state and all(state.get(k) == v for k, v in fields.items()) else None
                return wait_until(session, description, matches, timeout=10)

            def click(rect, *, left=False):
                x, y, width, height = rect
                x += min(10, width / 2) if left else width / 2
                y += height / 2
                print(f"Native click x={x:.1f}, y={y:.1f}", flush=True)
                command(session, "InjectWindowInput", event={"PointerMove": {"x": x, "y": y}})
                time.sleep(.08)
                for state in ("Pressed", "Released"):
                    command(session, "InjectWindowInput", event={"PointerButton": {
                        "button": "Primary", "state": state, "x": x, "y": y}})
                    time.sleep(.08)

            def control(name):
                return wait_until(session, name, lambda: card().get("controls", {}).get(name), timeout=10)

            def stable_rect(read, description):
                previous, repeated = None, 0
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline:
                    rect = read()
                    if rect and rect[3] >= 17 and rect == previous:
                        repeated += 1
                        if repeated >= 3:
                            return rect
                    else:
                        repeated = 0
                    previous = rect
                    time.sleep(.15)
                raise RuntimeError(f"unsettled or clipped landmark: {description}")

            def menu(name):
                return wait_until(session, name, lambda: inspect().get("menus", {}).get(name), timeout=10)

            def scroll_to_row(read, direction, description):
                # egui retains submenu scroll position across openings. Scroll
                # until the measured row is fully visible and stationary.
                for _ in range(12):
                    rect = read()
                    if rect and rect[3] >= 17:
                        return stable_rect(read, description)
                    x, y, w, h = menu("menu.huds.show_all")
                    command(session, "InjectWindowInput", event={"PointerMove": {
                        "x": x + w / 2, "y": y + h + 80}})
                    command(session, "InjectWindowInput", event={"Scroll": {
                        "x": 0.0, "y": direction * 3.0}})
                    time.sleep(.25)
                raise RuntimeError(f"{description} never scrolled into view")

            def close_huds():
                if inspect().get("menus", {}).get("menu.huds"):
                    click(menu("menu.view"))
                    wait_until(session, "View menu closed",
                               lambda: not inspect().get("menus", {}).get("menu.huds"), timeout=10)

            def open_huds():
                if inspect().get("menus", {}).get("menu.huds.show_all"):
                    return
                if not inspect().get("menus", {}).get("menu.huds"):
                    click(menu("menu.view"))
                rect = menu("menu.huds")
                x, y, width, height = rect
                command(session, "InjectWindowInput", event={"PointerMove": {
                    "x": x + width / 2, "y": y + height / 2}})
                menu("menu.huds.show_all")

            command(session, "ActivatePerspective", id="sandbox_view")
            command(session, "SetGuidedHudLayout", hud="visibility-probe",
                    title="Test instructions", width=360.0)
            command(session, "SetGuidedHudPerspectives", hud="visibility-probe",
                    perspectives=["sandbox_view"])
            command(session, "SetHint", hud="visibility-probe", text="A real native guidance card.")
            expect("initial", available=True, visible=True, collapsed=False, title="Test instructions")
            session.capture_screenshot(output / "initial.png")
            click(control("collapse"))
            expect("minimized", collapsed=True, visible=True)
            session.capture_screenshot(output / "minimized.png")
            click(control("collapse"))
            expect("expanded", collapsed=False)
            session.capture_screenshot(output / "expanded.png")

            click(control("dock"))
            expect("docked into the Guidance panel", docked=True, interactive=True)

            def separate_dock_regions():
                regions = inspect().get("regions", {})
                viewport, guidance = regions.get("viewport"), regions.get("guidance")
                return (viewport and guidance and viewport[2] > guidance[2]
                        and viewport[0] + viewport[2] <= guidance[0] + 1.0)

            def camera_inside_viewport():
                viewport = inspect().get("regions", {}).get("viewport")
                camera = runtime_hud()
                rect = camera.get("rect")
                return (camera.get("rendered") and viewport and rect
                        and rect[0] >= viewport[0] - 1
                        and rect[0] + rect[2] <= viewport[0] + viewport[2] + 1
                        and rect[1] >= viewport[1] - 1
                        and rect[1] + rect[3] <= viewport[1] + viewport[3] + 1)

            wait_until(session, "guidance beside, not over, the scene viewport",
                       separate_dock_regions, timeout=10)
            wait_until(session, "HTML camera HUD inside the scene, outside the Guidance dock",
                       camera_inside_viewport, timeout=10)
            viewport_docked_width = inspect()["regions"]["viewport"][2]
            session.capture_screenshot(output / "docked.png")
            click(control("collapse"))
            expect("minimized in dock", docked=True, collapsed=True)
            click(control("collapse"))
            expect("expanded in dock", docked=True, collapsed=False)
            click(control("dock"))
            expect("returned to floating card", docked=False, visible=True)
            def expanded_viewport():
                regions = inspect().get("regions", {})
                viewport = regions.get("viewport")
                return ("guidance" not in regions and viewport
                        and viewport[2] > viewport_docked_width + 50)

            wait_until(session, "empty guidance dock closed and viewport expanded after floating",
                       expanded_viewport, timeout=10)

            original = card()["hint"]
            click(control("hide"))
            expect("hidden by user", user_hidden=True, available=True, visible=False, hint=original)
            session.capture_screenshot(output / "hidden.png")
            open_huds()
            rect = control("menu")
            click(rect, left=True)
            expect("restored through menu", user_hidden=False, available=True, visible=True, hint=original)
            # Act on the menu's real native controls; showing cannot bypass authored scope.
            close_huds()
            graph = command(session, "CreateGraphHud")["hud"]

            def graph_visible(expected):
                return any(hud["hud"] == graph and hud["visible"] == expected
                           for hud in command(session, "InspectGraphHud")["huds"])

            command(session, "ActivatePerspective", id="analysis")
            expect("unavailable in other perspective", available=False, user_hidden=False, visible=False)
            open_huds()
            click(menu("menu.huds.hide_all"))
            expect("global hide", available=False, user_hidden=True, visible=False)
            wait_until(session, "native graph hidden", lambda: graph_visible(False), timeout=10)
            close_huds()
            open_huds()
            click(menu("menu.huds.show_all"))
            expect("global show retains authored scope", available=False, user_hidden=False, visible=False)
            wait_until(session, "native graph restored", lambda: graph_visible(True), timeout=10)
            close_huds()
            command(session, "RemoveGraphHud", hud=graph)
            command(session, "ActivatePerspective", id="sandbox_view")
            expect("available after perspective return", available=True, visible=True, user_hidden=False)
            session.capture_screenshot(output / "restored.png")

            # The shipped HTML camera status is an ordinary runtime HUD.
            # Its menu preference must not change the classroom Twin manifest.
            command(session, "OpenTwinAsset", manifest_asset="lunco://tutorials/classroom/twin.toml")

            def expect_runtime(description, timeout=15, **fields):
                def matches():
                    state = runtime_hud()
                    return state if state and all(state.get(k) == v for k, v in fields.items()) else None
                return wait_until(session, description, matches, timeout=timeout)

            def lab_card():
                return next((c for c in inspect()["cards"] if c.get("hud") == "lunar_rover_lab"), {})

            wait_until(session, "rover lesson startup",
                       lambda: lab_card().get("hint", "").startswith("1/6"))
            expect_runtime("initial HTML HUD", title="Camera status", enabled=True,
                           available=True, rendered=True)
            session.capture_screenshot(output / "rover-lab-float.png")
            open_huds()
            click(scroll_to_row(lambda: runtime_hud().get("menu"), -1,
                                "camera status menu"), left=True)
            expect_runtime("HTML HUD hidden", enabled=False, available=True, rendered=False)
            session.capture_screenshot(output / "html-hidden.png")
            open_huds()
            click(scroll_to_row(lambda: runtime_hud().get("menu"), -1,
                                "camera status restore row"), left=True)
            expect_runtime("HTML HUD restored", enabled=True, available=True, rendered=True)
            session.capture_screenshot(output / "html-restored.png")
            close_huds()
            # Dock the actual authored tutorial, not just the isolated probe.
            click(wait_until(session, "rover lab dock control",
                             lambda: lab_card().get("controls", {}).get("dock")))
            wait_until(session, "rover guidance and scene visible side by side",
                       lambda: lab_card().get("docked") and separate_dock_regions()
                       and camera_inside_viewport(), timeout=10)
            session.capture_screenshot(output / "rover-lab-docked.png")
            click(wait_until(session, "rover lab float control",
                             lambda: lab_card().get("controls", {}).get("dock")))
            wait_until(session, "rover guidance floated and dock closed",
                       lambda: not lab_card().get("docked") and "guidance" not in inspect().get("regions", {}),
                       timeout=10)
            click(wait_until(session, "rover lab hide control",
                             lambda: lab_card().get("controls", {}).get("hide")))
            wait_until(session, "rover guidance hidden", lambda: lab_card().get("user_hidden")
                       and not lab_card().get("visible"), timeout=10)
            command(session, "ActivatePerspective", id="analysis")
            wait_until(session, "rover hidden in another available perspective",
                       lambda: lab_card().get("available") and lab_card().get("user_hidden")
                       and not lab_card().get("visible"), timeout=10)
            command(session, "ActivatePerspective", id="sandbox_view")
            open_huds()
            click(scroll_to_row(lambda: lab_card().get("controls", {}).get("menu"), 1,
                                "rover lab menu row"), left=True)
            wait_until(session, "rover guidance restored without changing the lesson",
                       lambda: lab_card().get("visible") and not lab_card().get("user_hidden")
                       and lab_card().get("hint", "").startswith("1/6"), timeout=10)
            close_huds()

            # Run the authored positive and negative layout/title verdict
            # alongside the lesson without replacing its own script.
            source = command(session, "RunRhai", code='print(find_path("/RoverLab/BuildPad"));')
            target = int(source["stdout"].strip())
            offset = session.log_path.stat().st_size
            command(session, "RunScenarioAsset", target=target,
                    source_asset="lunco://scenarios/tests/guided_hud_layout.rhai", reload_policy="restart")
            wait_for_authored_verdict(session, title="GUIDED HUD LAYOUT",
                                      check_count=8, offset=offset, timeout_s=45)
            assert_render_log_clean(session.log_path)
    print("Native guidance HUD visibility: PASS; owned API session exited.", flush=True)


if __name__ == "__main__":
    main()
