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

            def tree():
                result = []
                pending = [root]
                visited = set()
                while pending and len(visited) < 1024:
                    path = pending.pop()
                    if path in visited:
                        continue
                    visited.add(path)
                    name = call(destination, path, "org.freedesktop.DBus.Properties", "Get", GLib.Variant("(ss)", ("org.a11y.atspi.Accessible", "Name")))[0]
                    result.append((path, name))
                    pending.extend(child for owner, child in call(destination, path, "org.a11y.atspi.Accessible", "GetChildren")[0] if owner == destination)
                return result

            def press_named(label):
                node = wait_for(session, lambda: next((path for path, name in tree() if name == label), None), "accessible " + label)
                accepted = call(destination, node, "org.a11y.atspi.Action", "DoAction", GLib.Variant("(i)", (0,)))[0]
                if not accepted:
                    raise RuntimeError("AT-SPI rejected action " + label)

            press_named("Controls")
            camera = wait_for(session, lambda: next((name for _, name in tree() if name.startswith("CAMERA")), None), "labelled authored control")
            press_named(camera)
            wait_for(session, lambda: any(name == "Image controls…" for _, name in tree()), "authored camera callback")
            session.capture_screenshot(area / "accessible-camera.png")
            (area / "camera-tree.json").write_text(json.dumps(tree(), indent=2))
            print("PASS: AT-SPI Controls menu invokes the exact authored camera callback", flush=True)
            for state in ("Pressed", "Released"):
                execute(session, "InjectWindowInput", event={"Key": {"key": "Escape", "state": state, "text": None, "repeat": False}})
            execute(session, "ActivatePerspective", id="editor")
            wait_for(session, lambda: execute(session, "InspectWorkbench")["active_perspective"] == "editor", "Editor perspective")
            press_named("Controls")
            wait_for(session, lambda: any(name == "No authored controls are visible." for _, name in tree()), "hidden-control withdrawal")
            session.capture_screenshot(area / "hidden-controls.png")
            print("PASS: hidden authored controls are withdrawn from the accessible menu", flush=True)
    finally:
        enabled(initial)
    print("PASS: owned process exited; desktop accessibility preference restored", flush=True)


if __name__ == "__main__":
    main()
