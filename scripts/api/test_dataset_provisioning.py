#!/usr/bin/env python3
"""Certify dataset input/lifecycle and prepared Twins in one owned window.

The driver serves deterministic local bytes and captures actual UI phases.
Rhai owns state/clock verdicts. Enter logical button coordinates from each
saved screenshot when prompted, or supply a recorded --clicks JSON mapping.
No authored USD is rewritten: test Twins are extracted package snapshots.
"""
from __future__ import annotations

import argparse
from contextlib import contextmanager
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import tarfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import zipfile

from runtime import BINARY, ROOT, ProductionSession, assert_render_log_clean, get_json


class Transfer:
    def __init__(self, payload: bytes, *, held=False, corrupt_first=False):
        self.payload = payload
        self.held = held
        self.corrupt_first = corrupt_first
        self.requests = 0
        self.started = threading.Event()
        self.release = threading.Event()
        if not held:
            self.release.set()


def execute(session, command, **params):
    response = session.post({"type": "ExecuteCommand", "command": command, "params": params})
    if response.get("error"):
        raise RuntimeError(f"{command}: {response}")
    return response.get("data", {})


def wait_for(predicate, description, timeout=150):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        last = predicate()
        if last:
            return last
        time.sleep(.2)
    raise RuntimeError(f"Timed out waiting for {description}; last={last}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4298)
    parser.add_argument("--output", type=Path, default=ROOT / "target/dataset-certification")
    parser.add_argument("--summer-zip", type=Path, default=ROOT / ".lunco/friend-twins/space-school-twin.zip")
    parser.add_argument("--griffin-zip", type=Path, default=ROOT / ".lunco/friend-twins/astrobotic-griffin-1.zip")
    parser.add_argument("--clicks", type=Path)
    parser.add_argument("--phase", choices=["all", "failure-and-twins", "twins", "griffin"], default="all")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    fixtures = output / "fixtures"
    if fixtures.exists():
        raise RuntimeError(f"Refusing to replace existing fixture inputs: {fixtures}")
    fixtures.mkdir()
    with zipfile.ZipFile(args.summer_zip) as package:
        package.extractall(fixtures / "cold")
        package.extractall(fixtures / "prepared")
    with zipfile.ZipFile(args.griffin_zip) as package:
        package.extractall(fixtures / "prepared")
    cold = fixtures / "cold/space-school-twin"
    terrain = cold / ".cache/terrain/apollo15_dem"
    payload = io.BytesIO()
    with tarfile.open(fileobj=payload, mode="w:gz", compresslevel=0) as archive:
        for path in sorted(terrain.rglob("*")):
            if path.is_file():
                archive.add(path, arcname=Path("prepared") / path.relative_to(terrain))
    shutil.rmtree(terrain)
    png = (cold / ".cache/terrain/apollo15/materials/textures/normal.png").read_bytes()
    transfers = {
        "/terrain.tar.gz": Transfer(payload.getvalue(), held=True),
        "/cancel.png": Transfer(png, held=True),
        "/retire.png": Transfer(png, held=True),
        "/retry.png": Transfer(png, corrupt_first=True),
    }

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *unused):
            pass

        def do_GET(self):
            transfer = transfers[self.path]
            transfer.requests += 1
            body = b"deliberate checksum failure" if transfer.corrupt_first and transfer.requests == 1 else transfer.payload
            start = int(self.headers.get("Range", "bytes=0-").split("=")[1].split("-")[0])
            self.send_response(206 if start else 200)
            self.send_header("Content-Length", str(len(body) - start))
            if start:
                self.send_header("Content-Range", f"bytes {start}-{len(body)-1}/{len(body)}")
            self.end_headers()
            try:
                for offset in range(start, len(body), 65536):
                    self.wfile.write(body[offset:offset + 65536])
                    self.wfile.flush()
                    transfer.started.set()
                    if not transfer.release.wait(120):
                        raise RuntimeError("transfer control timed out")
                    time.sleep(.03)
            except (BrokenPipeError, ConnectionResetError):
                pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    url = f"http://127.0.0.1:{server.server_port}"

    def declaration(key, path, dest, *, process=False, recommended=True):
        digest = hashlib.sha256(transfers[path].payload).hexdigest()
        text = (f'[{key}]\nname = "Certification {key}"\nurl = "{url}{path}"\n'
                f'dest = "{dest}"\nsha256 = "{digest}"\nrecommended = {str(recommended).lower()}\n')
        if process:
            text += (f'[{key}.process]\nkind = "texture"\noutput_root = "cache"\n'
                     f'output = "textures/{key}.png"\ntarget_resolution = [64, 64]\n')
        return text

    (cold / "Assets.toml").write_text(declaration("terrain", "/terrain.tar.gz", "terrain/apollo15_dem"))
    manifest = cold / "twin.toml"
    manifest.write_text(manifest.read_text().replace("suppress_missing_prompt = true", "suppress_missing_prompt = false"))
    cancel = fixtures / "cancel"
    cancel.mkdir()
    (cancel / "twin.toml").write_text('name = "DatasetTransferProbe"\nversion = "0.1.0"\n[usd]\ndefault_scene = "scene.usda"\n')
    shutil.copyfile(ROOT / "assets/scenes/base/lunar_surface.usda", cancel / "scene.usda")
    (cancel / "Assets.toml").write_text(
        declaration("cancel", "/cancel.png", "sources/cancel.png", process=True)
        + declaration("retire", "/retire.png", "sources/retire.png", process=True, recommended=False))
    retry = fixtures / "retry"
    retry.mkdir()
    (retry / "twin.toml").write_text('name = "DatasetFailureProbe"\nversion = "0.1.0"\n[usd]\ndefault_scene = "scene.usda"\n')
    shutil.copyfile(ROOT / "assets/scenes/base/lunar_surface.usda", retry / "scene.usda")
    (retry / "Assets.toml").write_text(declaration("retry", "/retry.png", "sources/retry.png", process=True))
    os.environ.update(LUNCOSIM_ISOLATED_RUN="1", LUNCOSIM_EPHEMERAL_SETTINGS="1",
                      LUNCOSIM_CONFIG=str(output / "config"))
    clicks = json.loads(args.clicks.read_text()) if args.clicks else {}
    assertions = (ROOT / "assets/scenarios/tests/dataset_provisioning_runtime.rhai").read_text()
    helpers = (ROOT / "assets/scripting/tests/lib/test_assert.rhai").read_text()
    verdicts = []
    log = output / "runtime.log"

    @contextmanager
    def release_transfers_before_exit():
        try:
            yield
        finally:
            for transfer in transfers.values():
                transfer.release.set()

    try:
        with ProductionSession(args.port, windowed=True, log_path=log,
                               extra_args=("--no-throttle", "--no-vsync")) as session, release_transfers_before_exit():
            pid = session.process.pid
            if Path(f"/proc/{pid}/cwd").resolve() != ROOT or Path(f"/proc/{pid}/exe").resolve() != BINARY.resolve():
                raise RuntimeError("session binary/cwd does not match the task checkout")
            print(f"OWNED pid={pid} port={args.port} cwd={ROOT}", flush=True)
            (output / "schema.json").write_text(json.dumps(session.post({"type": "DiscoverSchema"}), indent=2))

            def run(code):
                return execute(session, "RunRhai", code=code).get("stdout", "")

            def verdict(code):
                result = run(helpers + "\n" + assertions + "\n" + code)
                print(result.strip(), flush=True)
                if "TESTS_OK" not in result or "TESTS_FAIL" in result:
                    raise RuntimeError(f"Authored verdict failed: {result}")
                verdicts.append({"code": code, "stdout": result})
                (output / "verdicts.json").write_text(json.dumps(verdicts, indent=2))

            def scenario_gate(source, title):
                offset = log.stat().st_size
                execute(session, "RunScenarioAsset", source_asset=source)

                def terminal():
                    with log.open() as stream:
                        stream.seek(offset)
                        text = stream.read()
                    return text if title + ": PASS" in text or title + ": FAIL" in text else None

                result = wait_for(terminal, title)
                if title + ": FAIL" in result:
                    raise RuntimeError(result)
                verdicts.append({"scenario": source, "title": title, "verdict": "PASS"})
                (output / "verdicts.json").write_text(json.dumps(verdicts, indent=2))
                print(title + ": PASS", flush=True)

            def datasets(scope):
                return execute(session, "ListDatasets", scope=scope)["datasets"]

            def state(scope, key, kind):
                return next((row for row in datasets(scope) if row["key"] == key and row["state"]["kind"] == kind), None)

            def open_twin(path):
                previous = {row["scope"] for row in execute(session, "ListDatasets")["datasets"] if row["scope"] != "engine"}
                execute(session, "OpenTwin", path=str(path))
                wait_for(lambda: execute(session, "ListTwin").get("root") == str(path), "active Twin root")
                def incoming_scope():
                    scopes = {row["scope"] for row in execute(session, "ListDatasets")["datasets"] if row["scope"] != "engine"}
                    return next(iter(scopes)) if len(scopes) == 1 and scopes.isdisjoint(previous) else None
                scope = wait_for(incoming_scope, "fresh dataset scope")
                # Let the change-gated exposure projection publish the committed registry.
                time.sleep(.5)
                return scope

            def capture(name):
                path = output / f"{name}.png"
                session.capture_screenshot(path)
                print(f"CAPTURE {path}", flush=True)
                return path

            def click(name):
                capture(name)
                point = clicks.get(name)
                if point is None:
                    point = [float(value) for value in input(f"CLICK {name}: enter observed logical x y: ").split()]
                    if len(point) != 2:
                        raise RuntimeError("a click requires two coordinates")
                    clicks[name] = point
                (output / "clicks.json").write_text(json.dumps(clicks, indent=2))
                x, y = point
                for event in [
                    {"PointerMove": {"x": x, "y": y}},
                    {"PointerButton": {"button": "Primary", "state": "Pressed", "x": x, "y": y}},
                    {"PointerButton": {"button": "Primary", "state": "Released", "x": x, "y": y}},
                ]:
                    execute(session, "InjectWindowInput", event=event)
                    time.sleep(.08)

            if args.phase == "all":
                for filename, prefix in [("test_download_policies.rhai", ""),
                                         ("test_assets_policy.rhai", (ROOT / "assets/scripting/tools/assets.rhai").read_text())]:
                    result = run(helpers + "\n" + prefix + "\n" + (ROOT / "assets/scripting/tests" / filename).read_text())
                    (output / filename.replace(".rhai", ".txt")).write_text(result)
                    print(result.strip(), flush=True)
                    if "TESTS_OK" not in result or "TESTS_FAIL" in result:
                        raise RuntimeError(f"Policy gate failed: {filename}")
                execute(session, "DismissDatasetProvisioning", scope="engine")
                cold_scope = open_twin(cold)
                execute(session, "SetTimeTransport", playing=True)
                wait_for(lambda: "Simulation waiting for required terrain dataset" in run('print(clock_snapshot().simulation_progress);'), "terrain admission hold")
                tick = int(run('print(clock_snapshot().sim_tick);').strip())
                rover = int(run('print(find_path("/Traverse/Rover"));').strip())
                verdict(f'certify_terrain_hold("/terrain", {tick});')
                verdict(f'certify_dataset({json.dumps(cold_scope)}, "terrain", "missing", true, "request", "Not downloaded");')
                click("terrain-request-recommended")
                wait_for(lambda: state(cold_scope, "terrain", "downloading") and transfers["/terrain.tar.gz"].started.is_set(), "real UI download request")
                time.sleep(.3)
                verdict(f'certify_dataset({json.dumps(cold_scope)}, "terrain", "downloading", true, "cancel", "%");')
                verdict(f'certify_terrain_hold("/terrain", {tick});')
                capture("terrain-downloading")
                transfers["/terrain.tar.gz"].release.set()
                wait_for(lambda: state(cold_scope, "terrain", "installed"), "installed ground artifact")
                wait_for(lambda: get_json(args.port, "/api/ready")["data"]["ready"], "automatic terrain/model readiness")
                wait_for(lambda: int(run('print(clock_snapshot().sim_tick);').strip()) > tick + 60, "simulation ticks after installation")
                verdict(f'certify_resumed({tick}, "/Traverse/Rover", {rover});')
                verdict('certify_modelica(["/Traverse/Rover", "/Traverse/Rover/Electrical", "/Traverse/Rover/Antenna/YawHead/DishGimbal/DishHead/LinkAperture"]);')
                scenario_gate("twin://" + cold_scope.removeprefix("twin:") + "/sim/scenarios/tests/modelica_startup.rhai", "SUMMER MODELICA STARTUP")
                capture("terrain-ready")
                execute(session, "DismissDatasetProvisioning", scope=cold_scope)
                click("terrain-recent-events-open")
                time.sleep(.2)
                capture("terrain-recent-events")
                click("terrain-recent-events-close")

                cancel_scope = open_twin(cancel)
                click("cancel-request-recommended")
                wait_for(lambda: state(cancel_scope, "cancel", "downloading") and transfers["/cancel.png"].started.is_set(), "cancellable transfer")
                click("cancel-active-transfer")
                transfers["/cancel.png"].release.set()
                wait_for(lambda: state(cancel_scope, "cancel", "cancelled"), "cancelled worker")
                verdict(f'certify_dataset({json.dumps(cancel_scope)}, "cancel", "cancelled", true, "request", "Cancelled");')
                click("cancel-retry")
                wait_for(lambda: state(cancel_scope, "cancel", "installed"), "retry and texture processing")
                verdict(f'certify_dataset({json.dumps(cancel_scope)}, "cancel", "installed", true, "", "");')
                retiring = next(row for row in datasets(cancel_scope) if row["key"] == "retire")
                execute(session, "RequestDataset", id=retiring["id"])
                wait_for(lambda: transfers["/retire.png"].started.is_set(), "retiring transfer")
                retry_scope = open_twin(retry)
                transfers["/retire.png"].release.set()
                verdict(f'certify_retired({json.dumps(cancel_scope)});')
                if (cancel / ".cache/sources/retire.png").exists():
                    raise RuntimeError("retired transfer committed its outgoing source")
            elif args.phase == "failure-and-twins":
                execute(session, "DismissDatasetProvisioning", scope="engine")
                retry_scope = open_twin(retry)
            if args.phase not in ("twins", "griffin"):
                click("checksum-request-recommended")
                wait_for(lambda: state(retry_scope, "retry", "failed"), "checksum failure")
                verdict(f'certify_dataset({json.dumps(retry_scope)}, "retry", "failed", true, "request", "Failed");')
                execute(session, "SetTimeTransport", playing=False)
                time.sleep(.2)
                paused = int(run('print(clock_snapshot().sim_tick);').strip())
                click("checksum-retry")
                wait_for(lambda: state(retry_scope, "retry", "installed"), "successful checksum retry")
                verdict(f'certify_paused({paused});')
                verdict(f'certify_dataset({json.dumps(retry_scope)}, "retry", "installed", true, "", "");')
                capture("checksum-retry-ready")

            for name, paths in [
                ("space-school-twin", ["/Traverse/Rover", "/Traverse/Rover/Electrical", "/Traverse/Rover/Antenna/YawHead/DishGimbal/DishHead/LinkAperture"]),
                ("astrobotic-griffin-1", ["/Griffin1SurfaceOps/" + part for part in ["Lander", "Lander/IMU", "Lander/Altimeter/Model", "Lander/AttitudeReference", "Lander/GNC", "Lander/MainPropulsion", "Lander/AttitudeActuation", "Lander/AttitudePropulsion", "FLIP"]]),
            ]:
                if args.phase == "griffin" and name != "astrobotic-griffin-1":
                    continue
                scope = open_twin(fixtures / "prepared" / name)
                execute(session, "DismissDatasetProvisioning", scope=scope)
                execute(session, "SetTimeTransport", playing=True)
                def models_running():
                    participants = {row["name"]: row for row in execute(session, "CosimStatus")["entities"]}
                    if not set(paths).issubset(participants):
                        return False
                    for path in paths:
                        if participants[path]["modelica_error"]:
                            raise RuntimeError(f"{path}: {participants[path]['modelica_error']}")
                    return all(participants[path]["status"] == "Running" and participants[path]["modelica_current_time"] > 1.0 for path in paths)
                wait_for(models_running, name + " Modelica admission and solved state")
                wait_for(lambda: get_json(args.port, "/api/ready")["data"]["ready"], name + " readiness")
                wait_for(lambda: float(run('print(clock_snapshot().world_sim_s);').strip()) > 2.0, name + " simulation time")
                verdict("certify_modelica(" + json.dumps(paths) + ");")
                if name == "space-school-twin":
                    scenario_gate("twin://" + scope.removeprefix("twin:") + "/sim/scenarios/tests/modelica_startup.rhai", "SUMMER MODELICA STARTUP")
                else:
                    scenario_gate("twin://" + scope.removeprefix("twin:") + "/scenarios/tests/rumoca_startup.rhai", "GRIFFIN RUMOCA STARTUP")
                capture(name + "-ready")
                (output / (name + "-terrain.json")).write_text(json.dumps(execute(session, "TerrainLodStatus"), indent=2))
            assert_render_log_clean(log)
        (output / "verdicts.json").write_text(json.dumps(verdicts, indent=2))
        print(f"PASS phase={args.phase}: authored runtime verdicts, real UI input, API Exit and port release", flush=True)
    finally:
        for transfer in transfers.values():
            transfer.release.set()
        server.shutdown()
        server.server_close()


if __name__ == "__main__":
    main()
