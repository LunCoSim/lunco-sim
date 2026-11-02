#!/usr/bin/env python3
"""Certify Apollo startup data from pinned raw inputs through visible terrain."""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import shutil
import subprocess
import sys
import time
import tomllib
from urllib.parse import unquote, urlsplit

from runtime import BINARY, ROOT, ProductionSession, assert_render_log_clean, get_json
from test_asset_pipeline import execute, wait_for


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--twin",type=Path,required=True)
    parser.add_argument("--output",type=Path,required=True)
    parser.add_argument("--port",type=int,required=True)
    args=parser.parse_args()
    source=args.twin.resolve()
    output=args.output.resolve();output.mkdir(parents=True,exist_ok=False)
    twin=output/"SummerSpaceSchool";twin.mkdir()
    repo=Path(subprocess.check_output(["git","-C",str(source),"rev-parse","--show-toplevel"]).decode().strip())
    inventory=subprocess.check_output(["git","-C",str(repo),"ls-files","-z","--cached","--others","--exclude-standard","--",source.relative_to(repo).as_posix()]).decode().split("\0")
    for entry in set(inventory)-{""}:
        path=repo/entry
        if path.is_file():
            dest=twin/path.relative_to(source);dest.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(path,dest)
    manifest=tomllib.loads((source/"Assets.toml").read_text())
    for entry in manifest.values():
        if not entry.get("recommended",False):continue
        filename=unquote(Path(urlsplit(entry["url"]).path).name)
        candidates=list((source/".cache/sources").rglob(filename))
        if len(candidates)!=1:raise RuntimeError(f"provide the pinned raw input first: {filename}")
        path=candidates[0];dest=twin/path.relative_to(source)
        dest.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(path,dest)
    assertions=(ROOT/"assets/scripting/tests/lib/test_assert.rhai").read_text()
    gates=(ROOT/"assets/scenarios/tests/asset_pipeline.rhai").read_text()
    gates+=(ROOT/"assets/scenarios/tests/dataset_provisioning_runtime.rhai").read_text()
    verdicts=[]
    with ProductionSession(args.port,windowed=True,extra_args=["--no-throttle"],log_path=output/"runtime.log") as session:
        process=session.process
        assert Path(f"/proc/{process.pid}/cwd").resolve()==ROOT
        assert Path(f"/proc/{process.pid}/exe").resolve()==BINARY.resolve()
        print(f"OWNED SCHOOL pid={process.pid} port={args.port}",flush=True)
        def verdict(code):
            result=execute(session,"RunRhai",code=assertions+gates+"\n"+code)["stdout"]
            print(result.strip(),flush=True)
            if "TESTS_OK" not in result or "TESTS_FAIL" in result:raise RuntimeError(result)
            verdicts.append({"code":code,"stdout":result})
            (output/"verdicts.json").write_text(json.dumps(verdicts,indent=2))
        execute(session,"OpenTwin",path=str(twin))
        rows=wait_for(lambda:[r for r in execute(session,"ListDatasets").get("datasets",[]) if r["scope"].startswith("twin:SummerSpaceSchool")],"school manifest")
        scope=rows[0]["scope"]
        def held():
            report=execute(session,"RunRhai",code='let found=false;for b in clock_snapshot().simulation_progress.blockers {if b.owner=="SceneDatasets" || (b.owner=="TerrainPreparation" && b.reason.contains("apollo15_dtm")) {found=true;}} print(found);')
            return report["stdout"].strip()=="true"
        wait_for(held,"required terrain hold")
        wait_for(lambda: any(r["required_for_scene"] for r in execute(session,"ListDatasets").get("datasets",[]) if r["scope"]==scope),"composed scene requirements")
        tick=int(execute(session,"RunRhai",code="print(clock_snapshot().sim_tick);")["stdout"].strip())
        verdict('verify_runtime_download_catalog();')
        verdict(f'verify_scene_assets({json.dumps(scope)},["apollo15_dtm","apollo15_ortho","apollo15_normal","apollo15_slope"]);')
        verdict(f'certify_terrain_hold("apollo15_dtm",{tick});')
        verdict(f'verify_preparation_view({json.dumps(scope)},"apollo15_dtm","process","");')
        session.capture_screenshot(output/"startup-data.png")
        execute(session,"SetTimeTransport",playing=False)
        execute(session,"RunRhai",code=f'let result=assets::request_required({json.dumps(scope)});if result.ok==false {{throw result.error;}}')
        time.sleep(1)
        session.capture_screenshot(output/"preparing-data.png")
        def prepared():
            selected=[r for r in execute(session,"ListDatasets").get("datasets",[]) if r["scope"]==scope and r["required_for_scene"]]
            for row in selected:
                if row["state"]["kind"]=="failed":raise RuntimeError(str(row))
            return len(selected)==4 and all(r["state"]["kind"]=="installed" for r in selected)
        wait_for(prepared,"four startup artifacts",timeout=300)
        def ready():
            report=execute(session,"GetReadiness")
            if report.get("faulted"):raise RuntimeError(str(report))
            return report if report.get("ready") else None
        readiness=wait_for(ready,"school physics readiness",timeout=300)
        verdict(f'certify_paused({tick});')
        def visible():
            stream=execute(session,"TerrainLodStatus")["stream"]
            return stream["wanted"]>0 and stream["resident"]==stream["wanted"] and stream["pending"]==0
        wait_for(visible,"visible terrain after all bound images publish",timeout=180)
        verdict('verify_terrain_visible();')
        session.capture_screenshot(output/"data-ready-paused.png")
        execute(session,"DismissDatasetProvisioning",scope=scope)
        session.capture_screenshot(output/"terrain.png")
        (output/"readiness.json").write_text(json.dumps(readiness,indent=2))
        (output/"runtime-schema.json").write_text(json.dumps(get_json(args.port,"/api/commands/schema"),indent=2))
    assert process.returncode==0
    assert_render_log_clean(output/"runtime.log")
    print(f"PASS SCHOOL: {len(verdicts)} authored verdicts; owned session exited; evidence={output}",flush=True)


if __name__=="__main__":main()
