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

            def scroll_to_row(read, direction, description, group):
                # egui retains submenu scroll position across openings. Scroll
                # until the measured row is fully visible and stationary.
                for _ in range(12):
                    rect = read()
                    if rect and rect[3] >= 17:
                        return stable_rect(read, description)
                    x, y, w, h = menu(f"menu.huds.{group}")
                    command(session, "InjectWindowInput", event={"PointerMove": {
                        "x": x + w + 80, "y": y + h + 80}})
                    command(session, "InjectWindowInput", event={"Scroll": {
                        "x": 0.0, "y": direction * 3.0}})
                    time.sleep(.25)
                raise RuntimeError(f"{description} never scrolled into view")

            def close_huds():
                if inspect().get("menus", {}).get("menu.huds"):
                    click(menu("menu.view"))
                    wait_until(session, "View menu closed",
                               lambda: not inspect().get("menus", {}).get("menu.huds"), timeout=10)

            def open_huds(group=None):
                if not inspect().get("menus", {}).get("menu.huds.show_all"):
                    if not inspect().get("menus", {}).get("menu.huds"):
                        click(menu("menu.view"))
                    x, y, width, height = menu("menu.huds")
                    command(session, "InjectWindowInput", event={"PointerMove": {
                        "x": x + width / 2, "y": y + height / 2}})
                    menu("menu.huds.show_all")
                if group:
                    x, y, width, height = menu(f"menu.huds.{group}")
                    command(session, "InjectWindowInput", event={"PointerMove": {
                        "x": x + width / 2, "y": y + height / 2}})
                    time.sleep(.35)

            wait_until(session, "built-in camera listed before Twin open",
                       lambda: runtime_hud().get("group") == "builtin")
            if any(row["group"] == "twin" for row in command(session, "InspectRuntimeHuds")["huds"]):
                raise RuntimeError("Twin HUD rows leaked into the application catalog")
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
            open_huds("twin")
            rect = control("menu.show")
            click(rect)
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
            expect_runtime("initial HTML HUD", title="Camera status", group="builtin", enabled=True,
                           available=True, rendered=True)
            session.capture_screenshot(output / "rover-lab-float.png")
            open_huds("builtin")
            click(scroll_to_row(lambda: runtime_hud().get("menu_hide"), -1,
                                "camera status menu", "builtin"))
            expect_runtime("HTML HUD hidden", enabled=False, available=True, rendered=False)
            session.capture_screenshot(output / "html-hidden.png")
            open_huds("builtin")
            click(scroll_to_row(lambda: runtime_hud().get("menu_auto"), -1,
                                "camera status restore row", "builtin"))
            expect_runtime("HTML HUD restored", enabled=True, available=True, rendered=True, override=None)
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
            open_huds("twin")
            click(scroll_to_row(lambda: lab_card().get("controls", {}).get("menu.show"), 1,
                                "rover lab menu row", "twin"))
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
            # Crater Cup's floating signal grid is registered by its authored
            # perspective rather than the retained surface manifest.
            command(session, "OpenTwinAsset", manifest_asset="lunco://scenes/luncosim/twin.toml")
            def workbench():
                return command(session, "InspectWorkbench")
            def floating():
                return next((p["floating"][0] for p in workbench()["perspectives"]
                             if p["id"] == "twin/instruments" and p["floating"]), {})
            def grid_rendered():
                return any(panel["id"] == "signal_grid" for panel in workbench()["rendered_panels"])
            wait_until(session, "Crater Cup startup",
                       lambda: workbench().get("active_perspective") == "twin/instruments"
                       and runtime_hud().get("rendered") and grid_rendered(), timeout=180)
            open_huds("builtin")
            scroll_to_row(lambda: runtime_hud().get("menu_show"), -1, "built-in camera controls", "builtin")
            session.capture_screenshot(output / "built-in-huds.png")
            close_huds()
            open_huds("twin")
            scroll_to_row(lambda: floating().get("menu_show"), -1, "Twin floating grid controls", "twin")
            session.capture_screenshot(output / "twin-huds.png")
            for mode, override, rendered in (("hide", False, False), ("show", True, True), ("auto", None, True)):
                open_huds("twin")
                click(scroll_to_row(lambda: floating().get(f"menu_{mode}"), -1,
                                    f"floating grid {mode}", "twin"))
                wait_until(session, f"floating grid {mode} effective",
                           lambda: floating().get("override") == override and grid_rendered() is rendered)
            close_huds()
            # Native windows and image controls follow the same viewport gate
            # as the retained camera selector in the Twin's Instruments view.
            for view in command(session, "InspectCameraHudWindows")["views"]:
                command(session, "RemoveCameraFeed", feed=view["feed"])
            for view in command(session, "InspectGraphHud")["huds"]:
                command(session, "RemoveGraphHud", hud=view["hud"])
            feed = command(session, "CreateCameraFeed")["feed"]
            graph = command(session, "CreateGraphHud")["hud"]
            def camera_windows():
                return command(session, "InspectCameraHudWindows")
            def camera_window():
                return next((view for view in camera_windows()["views"] if view["feed"] == feed), {})
            def graph_window():
                return next((view for view in command(session, "InspectGraphHud")["huds"] if view["hud"] == graph), {})
            wait_until(session, "new native camera window actually rendered", lambda: camera_window().get("rect"), timeout=15)
            wait_until(session, "new native telemetry window actually rendered", lambda: graph_window().get("rendered"), timeout=15)
            pad_camera = int(command(session, "RunRhai", code='print(find_path("/World/PadCam"));')["stdout"].strip())
            plant = int(command(session, "RunRhai", code='print(find_path("/World/RelayPlant"));')["stdout"].strip())
            command(session, "SetCameraFeed", feed=feed, source=pad_camera, label="PadCam", width=640, height=360)
            command(session, "SetGraphHud", hud=graph, signals=[{"target_gid": plant, "signal": "process_w", "caption": "Refinery power"}], columns=1)
            wait_until(session, "selected camera feed ready and visible",
                       lambda: camera_window().get("rect") and any(view["feed"] == feed and view["ready"] for view in command(session, "InspectCameraFeeds")["views"]), timeout=20)
            wait_until(session, "bound telemetry graph ready and visible",
                       lambda: graph_window().get("rendered") and any(signal.get("samples", 0) for signal in graph_window().get("signals", [])), timeout=20)
            session.capture_screenshot(output / "crater-native-windows.png")
            # Use the same native retained-selector action as the operator.
            click(runtime_hud()["rect"])
            popup = wait_until(session, "camera selector popup", lambda: camera_windows().get("selector_popup"), timeout=15)
            click([popup[0] + 10, popup[1] + 8, 150, 20])
            command(session, "InjectWindowInput", event={"Key": {"key": "KeyI", "state": "Pressed", "text": "Image controls"}})
            command(session, "InjectWindowInput", event={"Key": {"key": "KeyI", "state": "Released"}})
            time.sleep(.5)
            popup = wait_until(session, "filtered image controls popup", lambda: camera_windows().get("selector_popup"), timeout=10)
            session.capture_screenshot(output / "crater-image-controls-option.png")
            click([popup[0] + 8, popup[1] + 63, 140, 20])
            wait_until(session, "camera image controls actually rendered", lambda: camera_windows().get("image_controls"), timeout=15)
            session.capture_screenshot(output / "crater-image-controls.png")
            camera_source = int(command(session, "RunRhai", code='print(find_path(current_camera_name()));')["stdout"].strip())
            def exposure():
                return command(session, "InspectCameraImageEffects", source=camera_source).get("exposure_ev100")
            exp_rect = wait_until(session, "actual fixed EV control", lambda: camera_windows().get("exposure_controls"), timeout=10)
            if exposure() is None:
                click([exp_rect[0], exp_rect[1], 20, 18])
            initial_ev = wait_until(session, "fixed exposure selected", lambda: [exposure()] if exposure() is not None else None, timeout=10)[0]
            exp_rect = wait_until(session, "visible numeric EV field", lambda: camera_windows().get("exposure_controls"), timeout=10)
            x, y = exp_rect[0] + 40, exp_rect[1] + exp_rect[3] - 9
            command(session, "InjectWindowInput", event={"PointerMove": {"x": x, "y": y}})
            command(session, "InjectWindowInput", event={"PointerButton": {"button": "Primary", "state": "Pressed", "x": x, "y": y}})
            time.sleep(.12)
            command(session, "InjectWindowInput", event={"PointerMove": {"x": x + 35, "y": y}})
            time.sleep(.12)
            command(session, "InjectWindowInput", event={"PointerButton": {"button": "Primary", "state": "Released", "x": x + 35, "y": y}})
            wait_until(session, "EV changed through native HUD", lambda: exposure() is not None and exposure() != initial_ev, timeout=10)
            session.capture_screenshot(output / "crater-ev-edited.png")
            exp_rect = wait_until(session, "EV reset row", lambda: camera_windows().get("exposure_controls"), timeout=10)
            click([exp_rect[0] + 95, exp_rect[1] + exp_rect[3] - 18, 65, 18])
            wait_until(session, "EV restored to inherited default by Reset EV", lambda: exposure() == initial_ev, timeout=10)
            session.capture_screenshot(output / "crater-ev-reset.png")
            command(session, "ActivatePerspective", id="modelica_analyze")
            wait_until(session, "native scene HUD windows unavailable outside a scene view",
                       lambda: not camera_window().get("rect") and not graph_window().get("rendered")
                       and not camera_windows().get("image_controls"), timeout=15)
            command(session, "ActivatePerspective", id="twin/instruments")
            wait_until(session, "native scene HUD windows restored with the viewport",
                       lambda: camera_window().get("rect") and graph_window().get("rendered")
                       and camera_windows().get("image_controls"), timeout=15)
            command(session, "RemoveCameraFeed", feed=feed)
            command(session, "RemoveGraphHud", hud=graph)
            with tempfile.TemporaryDirectory(prefix="hud-catalog-empty-") as empty:
                command(session, "OpenFolder", path=empty)
                wait_until(session, "Twin catalog retired",
                           lambda: not command(session, "ReadActiveTwinContract").get("open")
                           and not any(row["group"] == "twin" for row in command(session, "InspectRuntimeHuds")["huds"]))
                wait_until(session, "built-in camera retained", lambda: runtime_hud().get("group") == "builtin")
            assert_render_log_clean(session.log_path)
    print("Native guidance HUD visibility: PASS; owned API session exited.", flush=True)


if __name__ == "__main__":
    main()
