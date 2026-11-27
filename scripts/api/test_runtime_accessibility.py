#!/usr/bin/env python3
"""Check the owned Linux GUI's real AT-SPI menu and callback route.

Requires the system Python's PyGObject/Gio bindings and a desktop session.
Temporarily enables the desktop accessibility bus and restores its exact state.
"""
import argparse
import json
import os
from pathlib import Path
import time
import uuid

from gi.repository import Gio, GLib
from runtime import ROOT, BINARY, ProductionSession
from test_scene_loading_settings import execute, wait_for


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    args = parser.parse_args()
    area = ROOT / "target" / ("ui-accessibility-" + uuid.uuid4().hex)
    config = area / "config"
    config.mkdir(parents=True)
    (config / "settings.json").write_text(json.dumps({"window": {
        "width": 1280, "height": 800, "x": None, "y": None, "maximized": False,
    }}))
    os.environ["LUNCOSIM_CONFIG"] = str(config)
    os.environ.pop("LUNCOSIM_EPHEMERAL_SETTINGS", None)
    desktop = Gio.bus_get_sync(Gio.BusType.SESSION, None)

    def desktop_call(interface, method, parameters=None):
        return desktop.call_sync("org.a11y.Bus", "/org/a11y/bus", interface, method,
                                 parameters, None, Gio.DBusCallFlags.NONE, 5000, None).unpack()

    initial = desktop_call("org.freedesktop.DBus.Properties", "Get", GLib.Variant("(ss)", ("org.a11y.Status", "IsEnabled")))[0]

    def enabled(value):
        desktop_call("org.freedesktop.DBus.Properties", "Set", GLib.Variant("(ssv)", (
            "org.a11y.Status", "IsEnabled", GLib.Variant("b", value))))

    try:
        enabled(True)
        address = desktop_call("org.a11y.Bus", "GetAddress")[0]
        bus = Gio.DBusConnection.new_for_address_sync(address,
            Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION, None, None)

        def call(destination, path, interface, method, parameters=None):
            return bus.call_sync(destination, path, interface, method, parameters, None,
                                 Gio.DBusCallFlags.NONE, 5000, None).unpack()

        with ProductionSession(args.port, windowed=True, extra_args=("--no-throttle",), log_path=area / "runtime.log") as session:
            pid = session.process.pid
            if Path(f"/proc/{pid}/cwd").resolve() != ROOT or Path(f"/proc/{pid}/exe").resolve() != BINARY.resolve():
                raise RuntimeError("runtime does not belong to this checkout")
            print(f"OWNED pid={pid} port={args.port}; evidence={area}", flush=True)
            execute(session, "InjectWindowInput", event={"Focus": {"focused": True}})

            def application():
                children = call("org.a11y.atspi.Registry", "/org/a11y/atspi/accessible/root", "org.a11y.atspi.Accessible", "GetChildren")[0]
                for destination, path in children:
                    try:
                        owner = call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "GetConnectionUnixProcessID", GLib.Variant("(s)", (destination,)))[0]
                    except GLib.Error:
                        continue
                    if owner == pid:
                        return destination, path
                return None

            destination, root = wait_for(session, application, "owned AT-SPI application", timeout=30)

            def retired(error):
                return error.matches(Gio.dbus_error_quark(), Gio.DBusError.UNKNOWN_OBJECT)

            def tree():
                def snapshot():
                    result = []
                    pending = [root]
                    visited = set()
                    try:
                        while pending and len(visited) < 1024:
                            path = pending.pop()
                            if path in visited:
                                continue
                            visited.add(path)
                            name = call(destination, path, "org.freedesktop.DBus.Properties", "Get", GLib.Variant("(ss)", ("org.a11y.atspi.Accessible", "Name")))[0]
                            result.append((path, name))
                            pending.extend(child for owner, child in call(destination, path, "org.a11y.atspi.Accessible", "GetChildren")[0] if owner == destination)
                    except GLib.Error as error:
                        if retired(error):
                            # A modal replaced this tree revision. Discard the
                            # whole partial snapshot and acquire current leases.
                            return None
                        raise
                    return result
                return wait_for(session, snapshot, "current AT-SPI tree revision", timeout=15)

            def press_named(label):
                def activate():
                    node = next((path for path, name in tree() if name == label), None)
                    if node is None:
                        return None
                    try:
                        accepted = call(destination, node, "org.a11y.atspi.Action", "DoAction", GLib.Variant("(i)", (0,)))[0]
                    except GLib.Error as error:
                        if retired(error):
                            return None  # UnknownObject guarantees no action ran.
                        raise
                    if not accepted:
                        raise RuntimeError("AT-SPI rejected action " + label)
                    return True
                wait_for(session, activate, "accessible action " + label)

            press_named("Controls")
            camera = wait_for(session, lambda: next((name for _, name in tree() if name.startswith("CAMERA")), None), "labelled authored control")
            press_named(camera)
            wait_for(session, lambda: any(name == "Image controls…" for _, name in tree()), "authored camera callback")
            session.capture_screenshot(area / "accessible-camera.png")
            (area / "camera-tree.json").write_text(json.dumps(tree(), indent=2))
            print("PASS: AT-SPI Controls menu invokes the exact authored camera callback", flush=True)
            # The main image-controls window is independent of feed windows.
            # In particular an empty feed inventory must not suppress it.
            assert not execute(session, "InspectCameraFeeds")["views"]
            press_named("Image controls…")
            wait_for(session, lambda: any(name == "Camera image controls" for _, name in tree()),
                     "main camera image controls with no feeds")
            session.capture_screenshot(area / "main-camera-image-controls.png")
            print("PASS: main camera image controls open without a camera feed", flush=True)

            def close_menus():
                for _ in range(2):
                    for state in ("Pressed", "Released"):
                        execute(session, "InjectWindowInput", event={"Key": {"key": "Escape", "state": state, "text": None, "repeat": False}})

            def exposure_enabled(expected):
                saved = json.loads((config / "settings.json").read_text())
                section = saved.get("viewport_exposure")
                return section is not None and (section.get("ev100") is not None) == expected

            close_menus()
            press_named("Settings")
            press_named("General")
            press_named("Fixed viewport exposure")
            wait_for(session, lambda: exposure_enabled(True), "General sets the saved live viewport exposure")
            close_menus()
            press_named("Settings")
            press_named("Graphics")
            press_named("Camera look")
            press_named("Fixed viewport exposure")
            wait_for(session, lambda: exposure_enabled(False), "Graphics clears the same live exposure")
            print("PASS: General and Graphics edit the same persisted viewport EV100 override", flush=True)
            close_menus()
            execute(session, "ActivatePerspective", id="editor")
            wait_for(session, lambda: execute(session, "InspectWorkbench")["active_perspective"] == "editor", "Editor perspective")
            press_named("Controls")
            wait_for(session, lambda: any(name == "No authored controls are visible." for _, name in tree()), "hidden-control withdrawal")
            session.capture_screenshot(area / "hidden-controls.png")
            print("PASS: hidden authored controls are withdrawn from the accessible menu", flush=True)
            for state in ("Pressed", "Released"):
                execute(session, "InjectWindowInput", event={"Key": {"key": "Escape", "state": state, "text": None, "repeat": False}})
            created = execute(session, "CreateNewScratchModel", name="CloseProbe", source="model CloseProbe end CloseProbe;")
            doc_id = created["doc_id"]
            wait_for(session, lambda: any(panel["id"] == "modelica_model_view" for panel in execute(session, "InspectWorkbench")["rendered_panels"]), "owned model view")
            execute(session, "OpenInNewView", doc_id=doc_id)
            execute(session, "Exit")
            press_named("Cancel")
            wait_for(session, lambda: not any(name == "Don't save" for _, name in tree()), "close cancellation")
            session.capture_screenshot(area / "close-cancelled.png")
            if not any(doc["doc_id"] == doc_id for doc in execute(session, "ListOpenDocuments")["open_documents"]):
                raise RuntimeError("Cancel discarded the model")
            print("PASS: native close Cancel preserves the unsaved model", flush=True)
            output = execute(session, "RunRhai", code=(ROOT / "assets/scripting/tests/test_ui_review.rhai").read_text()
                + f"\nui_modelica_save_rejected({doc_id}, {json.dumps(str(area))});")["stdout"].strip()
            if "TESTS_FAIL" in output or ": PASS" not in output:
                raise RuntimeError(output)
            print(output, flush=True)
            execute(session, "Exit")
            press_named("Don't save")
            session.process.wait(timeout=15)
            print("PASS: native Don't save resolves the document and exits", flush=True)
    finally:
        enabled(initial)
    print("PASS: owned process exited; desktop accessibility preference restored", flush=True)


if __name__ == "__main__":
    main()
