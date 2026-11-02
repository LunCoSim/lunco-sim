#!/usr/bin/env python3
"""Certify scene-specific data admission, queue/cancel/retry and live scene edits.

The API authors the USD fixtures. Rhai supplies every runtime verdict; Python
only serves controlled bytes, sequences commands and records UI evidence.
"""
from __future__ import annotations
import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import io
import json
import os
from pathlib import Path
import shutil
import threading
import time
from PIL import Image
from runtime import ROOT, BINARY, ProductionSession, assert_render_log_clean, get_json, wait_for_authored_verdict
from test_asset_pipeline import execute, wait_for


def author_asset_references(session, path, references, *, close=True):
    execute(session,"OpenUsdSourceDocument",source=str(path))
    document=wait_for(lambda: next((row["doc_id"] for row in execute(session,"ListOpenDocuments")["open_documents"]
        if row["kind"]=="usd" and row["origin"]["kind"]=="file" and Path(row["origin"]["path"])==path),None),"fixture source document")
    def inspect():
        try:return execute(session,"InspectUsdDocument",doc_id=document,path="/LunarSurface")
        except RuntimeError as error:
            if "USD stage source asset is unavailable" in str(error):return None
            raise
    info=wait_for(inspect,"USD document source preparation")
    if not info["prim"]["exists"]:raise RuntimeError("fixture root is missing")
    target=info["layers"]["root"]["id"]
    execute(session,"ApplyUsdOps",doc_id=document,ops=[{"SetAttribute":{"edit_target":target,"path":"/LunarSurface","name":"dataset_"+str(index),"type_name":"asset","value":"@"+reference+"@"}} for index,reference in enumerate(references)])
    execute(session,"SaveDocument",doc_id=document)
    execute(session,"InspectUsdDocument",doc_id=document,path="/LunarSurface")
    if close:execute(session,"CloseDocument",doc_id=document)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port",type=int,required=True)
    parser.add_argument("--output",type=Path,required=True)
    parser.add_argument("--ui-phase",choices=["missing","second","fault"],default="second")
    parser.add_argument("--ui-review",action="store_true",help="retain the owned window until <output>/ui-reviewed exists")
    parser.add_argument("--exercise-controls",action="store_true",help="exercise the measured 1280x720 native preparation controls through pointer input")
    args=parser.parse_args()
    output=args.output.resolve();output.mkdir(parents=True,exist_ok=False)
    twin=output/"Scene Data Ж";twin.mkdir()
    (twin/"twin.toml").write_text('name="SceneDataProbe"\nversion="0.1.0"\n[usd]\ndefault_scene="first.usda"\nscenes=["*.usda"]\n')
    for name in ("first","second"):shutil.copyfile(ROOT/"assets/scenes/base/lunar_surface.usda",twin/(name+".usda"))
    image=io.BytesIO();Image.new("RGB",(2,2),(110,100,90)).save(image,format="PNG")
    payload=image.getvalue()+b"x"*1048576
    library=b"fn answer() { 42 }\n"
    broken_library=b"fn answer( { invalid source\n"
    release=threading.Event();requests={};lock=threading.Lock()
    class Handler(BaseHTTPRequestHandler):
        def log_message(self,*unused):pass
        def do_GET(self):
            with lock:
                requests[self.path]=requests.get(self.path,0)+1
                attempt=requests[self.path]
            body=library if self.path=="/lib.rhai" else (broken_library if self.path=="/broken.rhai" else (b"invalid source"+b"x"*1048576 if self.path=="/c.png" and attempt==1 else payload))
            self.send_response(200);self.send_header("Content-Length",str(len(body)));self.end_headers()
            try:
                self.wfile.write(body[:65536]);self.wfile.flush()
                release.wait(120)
                self.wfile.write(body[65536:])
            except (BrokenPipeError,ConnectionResetError):pass
    server=ThreadingHTTPServer(("127.0.0.1",0),Handler);server.daemon_threads=True
    threading.Thread(target=server.serve_forever,daemon=True).start()
    url=f"http://127.0.0.1:{server.server_port}"
    text=""
    for key in "abcd":
        text+=f'[{key}]\nname="Scene image {key}"\nurl="{url}/{key}.png"\ndest="sources/{key}.png"\nsha256="{hashlib.sha256(payload).hexdigest()}"\nrecommended=false\n[{key}.process]\nkind="texture"\noutput="textures/{key}.png"\ntarget_resolution=[2,2]\n'
    text+=f'[other]\nname="Other scene data"\nurl="{url}/other.bin"\ndest="data/other.bin"\nrecommended=true\n'
    text+=f'[script_library]\nname="On-demand Rhai library"\nurl="{url}/lib.rhai"\ndest="runtime/lib #100% Ж.rhai"\nsha256="{hashlib.sha256(library).hexdigest()}"\nrecommended=false\n'
    text+=f'[broken_library]\nname="Invalid on-demand Rhai library"\nurl="{url}/broken.rhai"\ndest="runtime/broken.rhai"\nsha256="{hashlib.sha256(broken_library).hexdigest()}"\nrecommended=false\n'
    (twin/"Assets.toml").write_text(text)
    (twin/"runtime").mkdir()
    assertions=(ROOT/"assets/scripting/tests/lib/test_assert.rhai").read_text()
    (twin/"runtime/dynamic.rhai").write_text(assertions+'\nfn on_start(me,ctx) { import "twin://SceneDataProbe/runtime/lib #100% Ж.rhai" as deferred; t_report([t_eq(deferred::answer(),42,"downloaded import executes through the normal resolver")]); print("DYNAMIC IMPORT: PASS"); }\n')
    (twin/"runtime/broken-consumer.rhai").write_text('fn on_start(me,ctx) { import "twin://SceneDataProbe/runtime/broken.rhai" as deferred; deferred::answer(); }\n')
    os.environ.update(LUNCOSIM_ISOLATED_RUN="1",LUNCOSIM_EPHEMERAL_SETTINGS="1",LUNCOSIM_CONFIG=str(output/"config"))
    helpers=(ROOT/"assets/scripting/tests/lib/test_assert.rhai").read_text()
    gates=(ROOT/"assets/scenarios/tests/asset_pipeline.rhai").read_text()
    verdicts=[]
    try:
        with ProductionSession(args.port,windowed=True,extra_args=["--no-throttle"],log_path=output/"runtime.log") as session:
            pid=session.process.pid
            if Path(f"/proc/{pid}/cwd").resolve()!=ROOT or Path(f"/proc/{pid}/exe").resolve()!=BINARY.resolve():raise RuntimeError("wrong owned session")
            print(f"OWNED SCENE DATA pid={pid} port={args.port}",flush=True)
            for name,refs in [("first",[f"textures/{key}.png" for key in "abcd"]),("second",["data/other.bin"])]:
                author_asset_references(session,twin/(name+".usda"),refs)
            def run(code):return execute(session,"RunRhai",code=code)["stdout"]
            def verdict(code):
                result=run(helpers+gates+"\n"+code);print(result.strip(),flush=True)
                if "TESTS_OK" not in result or "TESTS_FAIL" in result:raise RuntimeError(result)
                verdicts.append({"code":code,"stdout":result});(output/"verdicts.json").write_text(json.dumps(verdicts,indent=2))
            policy=(ROOT/"assets/scripting/tests/test_download_policies.rhai").read_text()
            result=run(helpers+policy);print(result.strip(),flush=True)
            if "TESTS_FAIL" in result or "TESTS_OK" not in result:raise RuntimeError(result)
            (output/"policy-verdict.txt").write_text(result)
            execute(session,"OpenTwin",path=str(twin))
            def rows():return [row for row in execute(session,"ListDatasets")["datasets"] if row["scope"].startswith("twin:SceneDataProbe")]
            catalog=wait_for(lambda: rows() or None,"Twin catalog");scope=catalog[0]["scope"]
            def selected(keys):return sorted(row["key"] for row in rows() if row["required_for_scene"])==keys
            wait_for(lambda:selected(list("abcd")),"first scene requirements")
            verdict(f'certify_scene_dependencies({json.dumps(scope)},["a","b","c","d"],true);')
            verdict('let status=query("GetReadiness",#{});t_report([t_true(status.application_available && !status.ready,"application stays available while simulation admission is held")]);')
            probe_source=helpers+'''\nfn apply(ctx) {
                let runtime=execution_context();
                t_report([t_eq(runtime.scope,"twin","Twin UI tool retains its registration owner"),
                    t_eq(runtime.cycle,"ui","tool runs on the UI cycle"),
                    t_eq(runtime.clock,"application","UI uses application time"),
                    t_eq(runtime.target_twin_id,runtime.owner_id,"Twin target survives independently of its route"),
                    t_true(query("GetReadiness",#{}).application_available && !query("GetReadiness",#{}).ready,"UI tool can run before simulation admission")]);
            }\n'''
            probe=execute(session,"RegisterToolLibrary",name="independent_ui_probe",source=probe_source)
            probe_result=execute(session,"RunRhaiToolHook",tool="independent_ui_probe",hook="apply",args={"Map":{}},owner_twin_id=probe["active_twin"])
            probe_output=probe_result["stdout"];print(probe_output.strip(),flush=True)
            if "TESTS_OK 5" not in probe_output or "TESTS_FAIL" in probe_output:raise RuntimeError(probe_output)
            verdicts.append({"scenario":"independent_ui_probe/apply","stdout":probe_output})
            session.capture_screenshot(output/"required-data.png")
            if args.ui_review and args.ui_phase=="missing":
                print("UI REVIEW READY "+str(output),flush=True)
                wait_for(lambda:(output/"ui-reviewed").exists(),"visual review completion",timeout=900)
            def click(x,y):
                run(f'input_pointer_move({x}.0,{y}.0);')
                run(f'input_click("primary",{x}.0,{y}.0);')
            if args.exercise_controls:
                def native_click(x,y):
                    for state in ["Pressed","Released"]:
                        execute(session,"InjectWindowInput",event={"PointerButton":{
                            "button":"Primary","state":state,"x":float(x),"y":float(y)}})
                        time.sleep(.03)
                native_click(25,18)
                session.capture_screenshot(output/"file-menu-during-preparation.png")
                native_click(200,100)
                click(480,290)
                wait_for(lambda:execute(session,"ReadExposures",surface="asset-consent")["surfaces"]["asset-consent"]["properties"]["rows"][0]["details_display"]=="flex","Details responds during scene preparation")
                verdict('let view=query("ReadExposures",#{surface:"asset-consent"}).surfaces["asset-consent"];t_report([t_true(view.properties.rows[0].details_display=="flex","native Details click works while scene admission is held")]);')
                session.capture_screenshot(output/"details-click.png")
                run('input_pointer_press("primary",480.0,290.0);')
                time.sleep(.2)
                run('input_pointer_release("primary",480.0,290.0);')
                wait_for(lambda:execute(session,"ReadExposures",surface="asset-consent")["surfaces"]["asset-consent"]["properties"]["rows"][0]["details_display"]=="none","collapse source details")
                verdict('let view=query("ReadExposures",#{surface:"asset-consent"}).surfaces["asset-consent"];t_report([t_true(view.properties.rows[0].details_display=="none","held press toggles details once and release does not invoke again")]);')
                click(810,215)
                wait_for(lambda:next(row for row in rows() if row["key"]=="a")["state"]["kind"]=="downloading","native Download button starts the selected asset")
                session.capture_screenshot(output/"download-click.png")
                click(840,215)
                wait_for(lambda:next(row for row in rows() if row["key"]=="a")["state"]["kind"]=="cancelling","native cancellation is admitted before completing the transfer")
                # Let the controlled server finish its current read so the
                # cooperative worker can acknowledge cancellation promptly.
                release.set()
                wait_for(lambda:next(row for row in rows() if row["key"]=="a")["state"]["kind"]=="cancelled","native Cancel button stops the selected asset")
                release.clear()
                verdict(f'let row=pipeline_dataset({json.dumps(scope)},"a");t_report([t_eq(row.state.kind,"cancelled","native download and cancel actions execute during scene loading")]);')
            execute(session,"SetTimeTransport",playing=True)
            if not args.exercise_controls:execute(session,"DismissDatasetProvisioning",scope=scope)
            tick=run('print(clock_snapshot().sim_tick);').strip()
            verdict('t_report([t_true(!query("GetReadiness",#{}).ready,"API readiness includes scene data hold")]);')
            if args.exercise_controls:
                click(700,611)
                wait_for(lambda:any(row["state"]["kind"] in ["queued","downloading"] for row in rows()),"native bulk download click")
                click(840,611)
                wait_for(lambda:not execute(session,"ReadExposures",surface="asset-consent")["surfaces"]["asset-consent"]["visible"],"native Keep scene paused click dismisses the window")
                verdict('let view=query("ReadExposures",#{surface:"asset-consent"}).surfaces["asset-consent"];t_report([t_true(!view.visible && !query("GetReadiness",#{}).ready,"native dismissal keeps missing scene data held")]);')
            else:execute(session,"RunRhai",code=f'let r=assets::request_required({json.dumps(scope)});if !r.ok {{throw r.error;}}')
            wait_for(lambda: any(row["state"]["kind"]=="queued" for row in rows()),"bounded preparation queue")
            queued=next(row for row in rows() if row["state"]["kind"]=="queued")
            verdict('let count=0;for d in query("ListDatasets",#{}).datasets {if d.state.kind=="downloading" {count+=1;}} t_report([t_true(count<=3,"configured preparation limit bounds active downloads")]);')
            execute(session,"CancelDataset",id=queued["id"])
            wait_for(lambda:any(row["id"]==queued["id"] and row["state"]["kind"]=="cancelled" for row in rows()),"queued cancellation")
            verdict(f't_report([t_eq(clock_snapshot().sim_tick,{tick},"dismissal and cancellation cannot release mandatory scene hold")]);')
            execute(session,"SetTimeTransport",playing=False)
            release.set()
            wait_for(lambda: next((row for row in rows() if row["key"]=="c" and row["state"]["kind"]=="failed"),None),"checksum failure")
            verdict(f'certify_scene_dependencies({json.dumps(scope)},["a","b","c","d"],true);')
            execute(session,"RunRhai",code=f'let r=assets::request_required({json.dumps(scope)});if !r.ok {{throw r.error;}}')
            wait_for(lambda:all(row["state"]["kind"]=="installed" for row in rows() if row["required_for_scene"]),"scene artifacts recover without restart")
            verdict(f'certify_scene_dependencies({json.dumps(scope)},["a","b","c","d"],false);')
            verdict(f'let other=pipeline_dataset({json.dumps(scope)},"other");t_report([t_true(other.recommended && !other.required_for_scene && other.state.kind=="missing","general Twin recommendations stay deferred")]);')
            verdict(f't_report([t_eq(clock_snapshot().sim_tick,{tick},"preparation preserves manual pause")]);')
            authority=scope.removeprefix("twin:")
            # Runtime source/import use derives the dependency without a manual
            # call to RequireAsset or any authored importance list.
            offset=session.log_path.stat().st_size
            execute(session,"RunScenarioAsset",source_asset=f"twin://{authority}/runtime/dynamic.rhai")
            wait_for(lambda:selected(["a","b","c","d","script_library"]),"runtime Rhai import requirement")
            verdict(f'certify_scene_dependencies({json.dumps(scope)},["a","b","c","d","script_library"],true);')
            verdict('t_report([t_true(!query("GetReadiness",#{}).faulted,"missing declared imports wait recoverably without faulting the scene")]);')
            verdict('let view=query("ReadExposures",#{surface:"asset-consent"}).surfaces["asset-consent"];t_report([t_true(view.visible,"new runtime dependency reopens a previously dismissed download window")]);')
            session.capture_screenshot(output/"dynamic-data-required.png")
            library_row=next(row for row in rows() if row["key"]=="script_library")
            execute(session,"RequestDataset",id=library_row["id"])
            wait_for(lambda:next(row for row in rows() if row["key"]=="script_library")["state"]["kind"]=="installed","dynamic library installation")
            wait_for(lambda:execute(session,"GetReadiness").get("ready"),"source graph recovers without restarting the request")
            verdict(f't_report([t_true(!clock_snapshot().transport_playing,"dynamic import recovery preserves manual pause"),t_eq(clock_snapshot().sim_tick,{tick},"source preparation holds ticks through import publication")]);')
            execute(session,"SetTimeTransport",playing=True)
            wait_for_authored_verdict(session,title="DYNAMIC IMPORT",check_count=1,offset=offset)
            verdicts.append({"scenario":"runtime/dynamic.rhai","stdout":"DYNAMIC IMPORT: PASS; TESTS_OK 1"})
            execute(session,"SetTimeTransport",playing=False)
            verdict(f'let result=assets::require("twin://{authority}/undeclared.bin");t_report([t_true(!result.ok && result.error.contains("Assets.toml"),"undeclared dynamic URI is rejected visibly")]);')
            execute(session,"RunRhai",code=f'let r=assets::require("twin://{authority}/data/other.bin");if !r.ok {{throw r.error;}}')
            wait_for(lambda:selected(["a","b","c","d","other","script_library"]),"explicit runtime URI use")
            verdict(f'certify_scene_dependencies({json.dumps(scope)},["a","b","c","d","other","script_library"],true);')
            execute(session,"LoadScene",path=f"twin://{authority}/second.usda")
            wait_for(lambda:selected(["other"]),"second scene requirements")
            verdict(f'certify_scene_dependencies({json.dumps(scope)},["other"],true);')
            session.capture_screenshot(output/"second-scene.png")
            author_asset_references(session,twin/"second.usda",["textures/a.png"],close=False)
            wait_for(lambda:selected(["a"]),"live asset edit replaces scene dependencies")
            verdict(f'certify_scene_dependencies({json.dumps(scope)},["a"],false);')
            author_asset_references(session,twin/"second.usda",["data/other.bin"],close=False)
            wait_for(lambda:selected(["other"]),"live asset edit acquires new missing data hold")
            verdict(f'certify_scene_dependencies({json.dumps(scope)},["other"],true);')
            if args.ui_review and args.ui_phase=="second":
                print("UI REVIEW READY "+str(output),flush=True)
                wait_for(lambda:(output/"ui-reviewed").exists(),"visual review completion",timeout=900)
            (output/"runtime-schema.json").write_text(json.dumps(session.post({"type":"DiscoverSchema"}),indent=2))
            author_asset_references(session,twin/"second.usda",["textures/a.png"],close=False)
            wait_for(lambda:selected(["a"]),"isolated source failure fixture")
            wait_for(lambda:execute(session,"GetReadiness").get("ready"),"source failure baseline")
            execute(session,"RunScenarioAsset",source_asset=f"twin://{authority}/runtime/broken-consumer.rhai")
            wait_for(lambda: "broken_library" in [row["key"] for row in rows() if row["required_for_scene"]],"invalid source declared before execution")
            verdict('t_report([t_true(!query("GetReadiness",#{}).faulted,"declared missing source remains recoverable before downloading")]);')
            broken=next(row for row in rows() if row["key"]=="broken_library")
            execute(session,"RequestDataset",id=broken["id"])
            wait_for(lambda:execute(session,"GetReadiness").get("faulted"),"fresh malformed downloaded source is terminal")
            verdict('let ready=query("GetReadiness",#{});t_report([t_true(ready.faulted && !ready.ready,"invalid downloaded source faults and holds simulation")]);')
            failed_tick=int(run("print(clock_snapshot().sim_tick);").strip())
            execute(session,"SetTimeTransport",playing=True)
            time.sleep(1)
            verdict(f't_report([t_eq(clock_snapshot().sim_tick,{failed_tick},"runtime source fault prevents physics even when play is requested")]);')
            (output/"invalid-source-readiness.json").write_text(json.dumps(execute(session,"GetReadiness"),indent=2))
            session.capture_screenshot(output/"invalid-source.png")
            if args.ui_review and args.ui_phase=="fault":
                print("UI REVIEW READY "+str(output),flush=True)
                wait_for(lambda:(output/"ui-reviewed").exists(),"visual review completion",timeout=900)
            execute(session,"ClearScene")
            wait_for(lambda:selected([]),"cleared requirements")
            verdict(f'certify_scene_dependencies({json.dumps(scope)},[],false);')
            (output/"requests.json").write_text(json.dumps(requests,indent=2))
    finally:release.set();server.shutdown();server.server_close()
    assert_render_log_clean(output/"runtime.log")
    print(f"PASS scene requirements, queue/cancel/retry, pause and scene replacement; {len(verdicts)} verdicts",flush=True)

if __name__=="__main__":main()
