#!/usr/bin/env python3
"""Check the production native cache owner across application and Twin settings."""

import argparse
import json
import os
from pathlib import Path
import time
import uuid

from runtime import BINARY, ROOT, ProductionSession, wait_for_authored_verdict

SCENE = "scenes/tests/modelica_initialization_network.usda"
SETTING = "modelica.native_code_cache_mib"


def execute(session, command, **params):
    response = session.post({"type": "ExecuteCommand", "command": command, "params": params})
    if response.get("error"):
        raise RuntimeError(f"{command}: {response['error']}")
    return response.get("data")


def open_twin(session, path):
    before = execute(session, "ReadActiveWorkspace").get("scope_id")
    execute(session, "OpenTwin", path=str(path))
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        current = execute(session, "ReadActiveWorkspace").get("scope_id")
        if current is not None and current != before:
            return
        time.sleep(.1)
    raise RuntimeError(f"{path} did not replace the active Twin")


def check_budget(session, mib, scene=None):
    offset = session.log_path.stat().st_size if scene else 0
    if scene:
        twin_id = execute(session, "ReadActiveWorkspace")["scope_id"]
        execute(session, "OpenTwinScene", twin_id=twin_id, relative_path=scene)
    wait_for_authored_verdict(
        session, title="MODELICA INITIALIZATION NETWORK", check_count=45, offset=offset,
    )
    source = ROOT / "assets/scenarios/tests/modelica_native_cache_settings.rhai"
    code = source.read_text() + f"\nverify_native_cache_capacity({mib});"
    verdict = execute(session, "RunRhai", code=code)["stdout"].strip()
    if "TESTS_OK" not in verdict or "TESTS_FAIL" in verdict:
        raise RuntimeError(f"native cache budget {mib} MiB failed: {verdict}")
    print(f"{mib} MiB: {verdict}", flush=True)


def save_fixture_scenes(session, override, replacement):
    fixture = next(
        doc["doc_id"] for doc in execute(session, "ListOpenDocuments")["open_documents"]
        if doc["title"] == Path(SCENE).stem and doc["kind"] == "usd"
    )
    execute(session, "InspectUsdDocument", doc_id=fixture, path="/ModelicaRuntimeContracts")
    scenes = {}
    for name in ("initial", "rejected", "edited", "reset", "before-replacement", "replacement"):
        root = replacement if name == "replacement" else override
        scene = root / name / Path(SCENE).name
        scene.parent.mkdir()
        execute(session, "SaveAsDocument", doc_id=fixture, path=str(scene))
        if not scene.is_file():
            raise RuntimeError(f"document owner did not save {scene}")
        scenes[name] = scene.relative_to(root).as_posix()
    return scenes


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--windowed", action="store_true")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True)
    config = output / "config"
    config.mkdir()
    (config / "settings.json").write_text(json.dumps({"modelica": {"native_code_cache_mib": 80}}))
    os.environ.update(LUNCOSIM_CONFIG=str(config), LUNCOSIM_ISOLATED_RUN="1")
    os.environ.pop("LUNCOSIM_EPHEMERAL_SETTINGS", None)

    override = output / "override"
    replacement = output / "replacement"
    for twin in (override, replacement):
        twin.mkdir()
        manifest = f'name = "{twin.name}"\nversion = "0.1.0"\nuuid = "{uuid.uuid4()}"\n'
        manifest += '\n[settings]\n"scene.load_default" = false\n'
        if twin == override:
            manifest += f'"{SETTING}" = 24\n'
        (twin / "twin.toml").write_text(manifest)

    with ProductionSession(args.port, windowed=args.windowed, extra_args=["--scene", SCENE],
                           log_path=output / "runtime.log") as session:
        process = Path(f"/proc/{session.process.pid}")
        if process.is_dir():
            assert (process / "cwd").resolve() == ROOT
            assert (process / "exe").resolve() == BINARY.resolve()
        check_budget(session, 80)
        scenes = save_fixture_scenes(session, override, replacement)
        open_twin(session, override)
        check_budget(session, 24, scenes["initial"])
        if args.windowed:
            execute(session, "FocusPanel", id="lunco.workbench.twin_settings")
            session.capture_screenshot(output / "twin-settings.png")
        try:
            execute(session, "SetTwinSetting", key=SETTING, value=0)
        except RuntimeError as error:
            if "requires an integer from" not in str(error):
                raise
        else:
            raise RuntimeError("out-of-range Twin cache setting was admitted")
        check_budget(session, 24, scenes["rejected"])
        execute(session, "SetTwinSetting", key=SETTING, value=128)
        check_budget(session, 128, scenes["edited"])
        execute(session, "ResetTwinSetting", key=SETTING)
        check_budget(session, 80, scenes["reset"])
        execute(session, "SetTwinSetting", key=SETTING, value=32)
        check_budget(session, 32, scenes["before-replacement"])
        open_twin(session, replacement)
        check_budget(session, 80, scenes["replacement"])
    print(f"Native cache settings passed; owned port {args.port} released")


if __name__ == "__main__":
    main()
