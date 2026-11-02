#!/usr/bin/env python3
"""Exercise manifest artifact recipes through one owned production session.

Only fixture creation, local byte delivery, artifact reads and API sequencing
live here. Behavioral verdicts are authored in asset_pipeline.rhai. Terrain
preparation itself always runs through LunCoSim's native processors.
"""
from __future__ import annotations

import argparse
import hashlib
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import shutil
import subprocess
import sys
import threading
import time
import tomllib

from PIL import Image
from runtime import BINARY, ROOT, ProductionSession, assert_render_log_clean, get_json


def execute(session, command, **params):
    reply = session.post({"type": "ExecuteCommand", "command": command, "params": params})
    if reply.get("error"):
        raise RuntimeError(f"{command}: {reply}")
    return reply.get("data", {})


def wait_for(check, label, timeout=180):
    end = time.monotonic() + timeout
    last = None
    while time.monotonic() < end:
        last = check()
        if last:
            return last
        time.sleep(.2)
    raise RuntimeError(f"timed out: {label}; last={last}")


def make_dynamic_fixture(root: Path, url: str, digest: str):
    root.mkdir()
    for directory in ("tools", "shaders", "models"):
        (root / directory).mkdir()
    (root / "twin.toml").write_text('name="ArtifactPipelineProbe"\nversion="0.1.0"\n[usd]\ndefault_scene="scene.usda"\n')
    shutil.copyfile(ROOT/"assets/scenes/base/lunar_surface.usda",root/"scene.usda")
    (root / "tools/plan.rhai").write_text(
        'fn plan(facts) { let plan=facts.plan; plan.steps[1].uniforms=[facts.settings.factor]; plan }\n')
    (root / "tools/copy.py").write_text('import shutil, sys, urllib.request\nshutil.copyfile(sys.argv[1],sys.argv[2])\nurllib.request.urlopen(sys.argv[3]).read()\n')
    (root / "shaders/kernel.wesl").write_text(
        'fn process_pixel(p: vec2<u32>, size: vec2<u32>, color: vec4<f32>) -> vec4<f32> {\n'
        'return vec4<f32>(color.rgb * parameter(0u), color.a);\n}\n')
    (root / "models/Calibration.mo").write_text(
        'model Calibration\nparameter Real gain=1;\noutput Real y;\nequation\ny=gain*time;\nend Calibration;\n')
    (root / "Assets.toml").write_text(f'''
[product]
name="Dynamic artifact probe"
url="{url}/input.png"
dest="sources/input.png"
sha256="{digest}"
recommended=true
[product.process]
kind="pipeline"
output="products/probe"
target_resolution=[2,2]
color_space="linear"
factor=0.5
[product.process.pipeline]
files={{planner="tools/plan.rhai",copier="tools/copy.py",kernel="shaders/kernel.wesl",model="models/Calibration.mo"}}
planner={{source="planner",entry="plan"}}
artifacts=[
 {{from="image",path="image.png",kind="image",dimensions=[2,2]}},
 {{from="physical",path="physical.toml",kind="file"}},
 {{from="copy",path="copy.png",kind="image",dimensions=[2,2]}}
]
[product.process.pipeline.sources.reference]
name="Additional declared input"
url="{url}/reference.png"
dest="sources/reference.png"
sha256="{digest}"
[[product.process.pipeline.steps]]
id="pixels"
input="source"
kind="texture"
output="pixels.png"
[[product.process.pipeline.steps]]
id="image"
input="pixels"
kind="shader"
output="image.png"
shader="kernel"
uniforms=[1.0]
input_color_space="linear"
output_color_space="linear"
timeout_s=30
[[product.process.pipeline.steps]]
id="physical"
input="model"
kind="modelica"
output="physical.toml"
class="Calibration"
timeout_s=60
bounds={{t_start=0.0,t_end=1.0,dt=0.25,tolerance=0.000001}}
overrides={{gain=2.0}}
[[product.process.pipeline.steps]]
id="copy"
input="image"
kind="external"
output="copy.png"
program={json.dumps(sys.executable)}
tool_version={json.dumps(sys.version.split()[0])}
args=["{{copier}}","{{input}}","{{output}}","{url}/processed"]
timeout_s=10
''')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--griffin", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    raw = output / "raw"
    raw.mkdir()
    image = Image.new("RGBA", (2, 2))
    image.putdata([(100, 80, 60, 255), (20, 40, 80, 255), (200, 160, 120, 255), (240, 200, 160, 255)])
    image.save(raw / "input.png")
    shutil.copyfile(raw / "input.png", raw / "reference.png")
    requests = []

    class Handler(SimpleHTTPRequestHandler):
        def __init__(self, *a, **kw):
            super().__init__(*a, directory=str(raw), **kw)

        def log_message(self, *unused):
            pass

        def do_GET(self):
            requests.append(self.path)
            if self.path=="/processed":
                self.send_response(200)
                self.end_headers()
                self.wfile.write(b"ok")
                return
            super().do_GET()

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    url = f"http://127.0.0.1:{server.server_port}"
    dynamic = output / "dynamic"
    make_dynamic_fixture(dynamic, url, hashlib.sha256((raw / "input.png").read_bytes()).hexdigest())
    retired = output / "retired"
    retired.mkdir()
    (retired / "twin.toml").write_text('name="RetirementProbe"\nversion="0.1.0"\n[usd]\ndefault_scene="scene.usda"\n')
    shutil.copyfile(ROOT/"assets/scenes/base/lunar_surface.usda",retired/"scene.usda")
    assertions = (ROOT / "assets/scripting/tests/lib/test_assert.rhai").read_text()
    gates = (ROOT / "assets/scenarios/tests/asset_pipeline.rhai").read_text()
    gates += (ROOT / "assets/scenarios/tests/dataset_provisioning_runtime.rhai").read_text()
    verdicts = []
    log = output / "runtime.log"
    try:
        with ProductionSession(args.port, windowed=True, log_path=log) as session:
            process=session.process
            pid = session.process.pid
            if Path(f"/proc/{pid}/cwd").resolve() != ROOT or Path(f"/proc/{pid}/exe").resolve() != BINARY.resolve():
                raise RuntimeError("owned process does not match the production binary/checkout")
            print(f"OWNED pid={pid} port={args.port}", flush=True)
            (output/"runtime-schema.json").write_text(json.dumps(get_json(args.port,"/api/commands/schema"),indent=2))

            def verdict(code):
                stdout = execute(session, "RunRhai", code=assertions + "\n" + gates + "\n" + code).get("stdout", "")
                print(stdout.strip(), flush=True)
                if "TESTS_OK" not in stdout or "TESTS_FAIL" in stdout:
                    raise RuntimeError(f"authored verdict failed: {stdout}")
                verdicts.append({"code": code, "stdout": stdout})
                (output / "verdicts.json").write_text(json.dumps(verdicts, indent=2))

            def dataset(scope, key):
                return next((row for row in execute(session, "ListDatasets").get("datasets", []) if row["scope"] == scope and row["key"] == key), None)

            def capture_view(scope,key,action,name,error=""):
                row=dataset(scope,key)
                def projected():
                    surfaces=execute(session,"ReadExposures",surface="asset-consent").get("surfaces",{})
                    return next((r for r in surfaces.get("asset-consent",{}).get("properties",{}).get("rows",[])
                        if r["id"]==row["id"] and r["action"]==action),None)
                wait_for(projected,"prepared dataset view")
                verdict(f'verify_preparation_view({json.dumps(scope)},{json.dumps(key)},{json.dumps(action)},{json.dumps(error)});')
                for _ in range(10):
                    execute(session,"ReadExposures",surface="asset-consent")
                    time.sleep(.15)
                session.capture_screenshot(output/(name+".png"))

            def terminal(scope, key, expected):
                row = dataset(scope, key)
                if row and row["state"]["kind"] == "failed" and expected != "failed":
                    raise RuntimeError(str(row["state"]))
                return row if row and row["state"]["kind"] == expected else None

            from test_scene_asset_requirements import author_asset_references
            author_asset_references(session,dynamic/"scene.usda",["products/probe/image.png"])

            def open_twin(path, scope, key):
                execute(session, "OpenTwin", path=str(path))
                wait_for(lambda: execute(session,"ListTwin").get("root") == str(path), "active Twin root")
                return wait_for(lambda: next((row for row in execute(session,"ListDatasets").get("datasets",[])
                    if row["scope"].startswith(scope) and row["key"]==key),None), "dataset manifest admission")

            scope = "twin:ArtifactPipelineProbe"
            row = open_twin(dynamic, scope, "product")
            scope = row["scope"]
            verdict((ROOT/"assets/scripting/tests/test_download_policies.rhai").read_text())
            verdict('verify_runtime_download_catalog();')
            verdict('let result=cmd("OpenWebUrl",#{url:"file:///tmp/not-a-web-link"}); t_report([t_true(result.ok==false,"source links reject non-web addresses before browser dispatch")]);')
            verdict(f'verify_pipeline("{scope}","product","missing",3,4);')
            capture_view(scope,"product","request","missing-sources")
            execute(session, "RequestDataset", id=row["id"])
            wait_for(lambda: terminal(scope, "product", "installed"), "dynamic code artifact preparation")
            verdict(f'verify_pipeline("{scope}","product","installed",3,4);')
            capture_view(scope,"product","dismiss","prepared-data")
            execute(session,"ProcessDataset",id=row["id"],force=False)
            wait_for(lambda:terminal(scope,"product","installed"),"cached preparation")
            verdict("verify_force_runs("+json.dumps(requests)+",1);")
            execute(session,"ProcessDataset",id=row["id"],force=True)
            wait_for(lambda:terminal(scope,"product","installed"),"forced rebuild")
            verdict("verify_force_runs("+json.dumps(requests)+",2);")
            artifact = dynamic / ".cache/products/probe"
            result = tomllib.loads((artifact / "physical.toml").read_text())
            before = Image.open(raw / "input.png").convert("RGBA").tobytes()
            after = Image.open(artifact / "image.png").convert("RGBA").tobytes()
            verdict("verify_generated_data(" + ",".join(json.dumps(value) for value in (
                list(before), list(after), .5, result["times"], result["series"]["y"], 2.0, .000001)) + ");")
            verdict("verify_preserved_artifact(" + json.dumps((artifact / "image.png").read_bytes().hex()) + "," + json.dumps((artifact / "copy.png").read_bytes().hex()) + ");")
            old = (artifact / "image.png").read_bytes().hex()
            planner=dynamic/"tools/plan.rhai"
            valid_planner=planner.read_text()
            planner.write_text('fn plan(facts) { let plan=facts.plan; plan.steps[1].target_resolution=[4294967295,4294967295]; plan }\n')
            execute(session,"ProcessDataset",id=row["id"],force=True)
            wait_for(lambda:terminal(scope,"product","failed"),"oversized shader rejection")
            verdict(f'verify_pipeline_failure("{scope}","product","addressable output size");')
            capture_view(scope,"product","process","oversized-shader","addressable output size")
            verdict("verify_preserved_artifact("+json.dumps(old)+","+json.dumps((artifact/"image.png").read_bytes().hex())+");")
            planner.write_text(valid_planner)
            execute(session,"ProcessDataset",id=row["id"],force=False)
            wait_for(lambda:terminal(scope,"product","installed"),"valid recipe after shader rejection")
            verdict(f'verify_pipeline("{scope}","product","installed",3,4);')
            def retire():
                execute(session, "OpenTwin", path=str(retired))
                wait_for(lambda: not any(d["scope"]==scope for d in execute(session,"ListDatasets").get("datasets",[])), "Twin retirement")

            retire()
            (dynamic / ".cache/sources/reference.png").unlink()
            row = open_twin(dynamic,"twin:ArtifactPipelineProbe","product")
            scope = row["scope"]
            verdict(f'verify_pipeline("{scope}","product","missing",3,4);')
            execute(session,"RequestDataset",id=row["id"])
            wait_for(lambda: terminal(scope,"product","installed"),"additional source recovery with primary cached")
            verdict(f'verify_pipeline("{scope}","product","installed",3,4);')
            verdict("verify_source_requests("+json.dumps(requests)+");")
            retire()
            shadow = dynamic / "products/probe"
            shadow.mkdir(parents=True)
            (shadow / "image.png").write_bytes(b"incomplete authored override")
            row = open_twin(dynamic,"twin:ArtifactPipelineProbe","product")
            scope = row["scope"]
            execute(session,"ProcessDataset",id=row["id"])
            wait_for(lambda: terminal(scope,"product","failed"),"consumer override validation")
            verdict(f'verify_pipeline_failure("{scope}","product","Authored override");')
            capture_view(scope,"product","process","invalid-override","Authored override")
            retire()
            shutil.rmtree(shadow)
            # The generated fixture is unmounted and has no open source buffer.
            (dynamic / "tools/plan.rhai").write_text('fn plan(facts) { throw "intentional pipeline rejection"; }\n')
            row = open_twin(dynamic, "twin:ArtifactPipelineProbe", "product")
            scope = row["scope"]
            verdict(f'verify_pipeline("{scope}","product","missing",3,4);')
            execute(session, "ProcessDataset", id=row["id"])
            wait_for(lambda: terminal(scope, "product", "failed"), "terminal planner rejection")
            verdict(f'verify_pipeline_failure("{scope}","product","intentional pipeline rejection");')
            capture_view(scope,"product","process","failed-processing","intentional pipeline rejection")
            verdict("verify_preserved_artifact(" + json.dumps(old) + "," + json.dumps((artifact / "image.png").read_bytes().hex()) + ");")
        if process.returncode!=0:raise RuntimeError(f"production session exited with {process.returncode}: {log}")
        assert_render_log_clean(log)

        log = output / "griffin-runtime.log"
        with ProductionSession(args.port, windowed=True, log_path=log) as session:
            process=session.process
            pid = session.process.pid
            if Path(f"/proc/{pid}/cwd").resolve() != ROOT or Path(f"/proc/{pid}/exe").resolve() != BINARY.resolve():
                raise RuntimeError("owned process does not match the production binary/checkout")
            print(f"OWNED TERRAIN pid={pid} port={args.port}", flush=True)
            griffin = output / "griffin"
            griffin.mkdir()
            repo = args.griffin.resolve().parents[1]
            relative = args.griffin.resolve().relative_to(repo).as_posix()
            inventory=subprocess.check_output(["git","-C",str(repo),"ls-files","-z","--cached","--others","--exclude-standard","--",relative]).decode().split("\0")
            for entry in sorted(set(inventory)-{""}):
                source_path=repo/entry
                if not source_path.is_file():continue
                destination=griffin/source_path.relative_to(args.griffin.resolve())
                destination.parent.mkdir(parents=True,exist_ok=True)
                shutil.copy2(source_path,destination)
            source = args.griffin / ".cache/sources/nobile03"
            shutil.copytree(source, griffin / ".cache/sources/nobile03")
            if (griffin / "terrain").exists():
                raise RuntimeError("source archive unexpectedly contains generated authored terrain")
            scope = "twin:astrobotic-griffin-1"
            row = open_twin(griffin, scope, "nobile03_dem")
            scope = row["scope"]
            verdict(f'verify_pipeline("{scope}","nobile03_dem","missing",3,3);')
            wait_for(lambda: dataset(scope,"nobile03_dem")["required_for_scene"],
                "composed terrain dataset requirement")
            capture_view(scope,"nobile03_dem","process","downloaded-unprepared")
            clock=execute(session,"RunRhai",code="print(clock_snapshot().sim_tick);")["stdout"].strip()
            tick=int(clock.splitlines()[-1])
            time.sleep(.5)
            verdict(f'certify_terrain_hold("nobile03_dem",{tick});')
            execute(session,"SetTimeTransport",playing=False)
            execute(session, "RequestDataset", id=row["id"])
            wait_for(lambda: terminal(scope, "nobile03_dem", "installed"), "native NOBILE03 preparation")
            verdict(f'verify_pipeline("{scope}","nobile03_dem","installed",3,3);')
            wait_for(lambda: execute(session,"TerrainHeights",points=[[0.0,0.0]])[0].get("found"), "terrain admission after artifact publication")
            verdict(f'verify_pipeline_terrain("{scope}","nobile03_dem","twin://astrobotic-griffin-1/terrain/nobile03");')
            def scene_ready():
                readiness=execute(session,"GetReadiness")
                if readiness.get("faulted"):
                    (output/"readiness-failure.json").write_text(json.dumps(readiness,indent=2))
                    raise RuntimeError("Griffin readiness failed: "+str(readiness["fault"]))
                return readiness.get("ready")
            wait_for(scene_ready,"complete Griffin runtime readiness")
            library=execute(session,"RegisterToolLibrary",name="flip_panel_visibility",
                source=assertions+"\n"+(griffin/"scenarios/tests/flip_panel_visibility.rhai").read_text())
            panel=execute(session,"RunRhaiToolHook",tool="flip_panel_visibility",hook="apply",
                args={"Map":{"root":{"String":"/Griffin1SurfaceOps/FLIP"}}},owner_twin_id=library["active_twin"])
            print(panel["stdout"].strip(),flush=True)
            if "TESTS_OK 4" not in panel["stdout"] or "TESTS_FAIL" in panel["stdout"]:raise RuntimeError(panel)
            verdicts.append({"scenario":"flip_panel_visibility/apply","stdout":panel["stdout"]})
            (output/"verdicts.json").write_text(json.dumps(verdicts,indent=2))
            verdict(f'certify_paused({tick});')
            entity=int(execute(session,"RunRhai",code='print(find_path("/Griffin1SurfaceOps/Lander"));')["stdout"].strip())
            execute(session,"SetTimeTransport",playing=True)
            wait_for(lambda:int(execute(session,"RunRhai",code="print(clock_snapshot().sim_tick);")["stdout"].strip())>tick,"simulation resumes after resident terrain admission")
            verdict(f'certify_resumed({tick},"/Griffin1SurfaceOps/Lander",{entity});')
            execute(session,"SetTimeTransport",playing=False)
            execute(session,"DismissDatasetProvisioning",scope=scope)
            execute(session,"SetUserCamera",name="/Griffin1SurfaceOps/Avatar")
            def settled_terrain():
                state=execute(session,"TerrainLodStatus")
                stream=state["stream"]
                return state if stream["wanted"]>0 and stream["wanted"]==stream["resident"] and stream["pending"]==0 else None
            lod=wait_for(settled_terrain,"terrain visual cover")
            (output / "terrain-lod.json").write_text(json.dumps(lod,indent=2))
            time.sleep(2)
            session.capture_screenshot(output / "griffin-terrain.png")
            (output / "readiness.json").write_text(json.dumps(execute(session,"GetReadiness"), indent=2))
            (output / "requests.json").write_text(json.dumps(requests, indent=2))
    finally:
        server.shutdown()
        server.server_close()
    assert_render_log_clean(log)
    if process.returncode!=0:raise RuntimeError(f"terrain session exited with {process.returncode}: {log}")
    print(f"PASS: {len(verdicts)} authored verdicts; owned session exited; evidence={output}", flush=True)


if __name__ == "__main__":
    main()
