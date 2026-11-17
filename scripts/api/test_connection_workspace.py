#!/usr/bin/env python3
"""Exercise connection views, native wiring and live status in an owned window."""
import argparse
import json
import os
from pathlib import Path
import time
import uuid

from runtime import ROOT, BINARY, ProductionSession, assert_render_log_clean
from test_scene_loading_settings import execute, wait_for, rejection

ASSERTIONS = ROOT / "assets/scripting/tests/test_connection_workspace.rhai"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--compact", action="store_true")
    args = parser.parse_args()
    area = ROOT / "target" / ("connection-workspace-" + uuid.uuid4().hex)
    config = area / "config"
    config.mkdir(parents=True)
    twin_root = area / "twin"
    twin_root.mkdir()
    width, height = (960, 540) if args.compact else (1440, 900)
    (config / "settings.json").write_text(json.dumps({"window": {
        "width": width, "height": height, "x": None, "y": None, "maximized": False,
    }}))
    os.environ["LUNCOSIM_CONFIG"] = str(config)
    os.environ.pop("LUNCOSIM_EPHEMERAL_SETTINGS", None)
    print(f"Evidence: {area}", flush=True)
    log = area / "runtime.log"
    with ProductionSession(args.port, windowed=True, extra_args=("--no-throttle",), log_path=log) as session:
        pid = session.process.pid
        if Path(f"/proc/{pid}/cwd").resolve() != ROOT or Path(f"/proc/{pid}/exe").resolve() != BINARY.resolve():
            raise RuntimeError("owned runtime does not match this checkout")
        print(f"OWNED pid={pid} port={args.port}", flush=True)

        def cmd(command_name, **params):
            return execute(session, command_name, **params)

        def gate(expression):
            output = cmd("RunRhai", code=ASSERTIONS.read_text() + "\n" + expression)["stdout"]
            print(output.strip(), flush=True)
            if "TESTS_FAIL" in output or ": PASS" not in output:
                raise RuntimeError(output)

        def diagram():
            return cmd("InspectConnectionDiagram")

        def capture(name):
            cmd("InjectWindowInput", event={"PointerMove": {"x": width - 10, "y": height - 20}})
            time.sleep(5)  # Let verdict toasts retire before visual inspection.
            session.capture_screenshot(area / name)

        def workspace(mode, **params):
            cmd("SetConnectionWorkspace", view_id=diagram()["view_id"], presentation=mode, **params)
            wait_for(session, lambda: diagram().get("presentation") == mode, f"{mode} view")

        try:
            policy = cmd("RunRhai", code=(ROOT / "assets/scripting/tests/test_diagram_layout.rhai").read_text())["stdout"]
            print(policy.strip(), flush=True)
            if "TESTS_FAIL" in policy or "DIAGRAM LAYOUT: PASS" not in policy:
                raise RuntimeError(policy)
            cmd("ActivatePerspective", id="editor")
            cmd("NewDocument", kind="usd")
            doc = wait_for(session, lambda: next((d["doc_id"] for d in cmd("ListOpenDocuments")["open_documents"]
                            if d["kind"] == "usd" and d["origin"].get("kind") == "untitled"), None), "new USD document")
            def document_ready():
                try:
                    return cmd("InspectUsdDocument", doc_id=doc, path="/World")["prim"]["exists"]
                except RuntimeError as error:
                    if "USD stage is loading" in str(error):
                        return False
                    raise
            wait_for(session, document_ready, "USD stage")
            ops = []
            for name in ("Source", "Sink"):
                path = "/World/" + name
                ops += [{"AddPrim": {"edit_target": "@root@", "parent_path": "/World", "name": name,
                                    "type_name": "Scope", "reference": None, "reference_prim_path": None}},
                        {"SetApiSchemas": {"edit_target": "@root@", "path": path, "schemas": ["LunCoProgramAPI"]}}]
                for attr, kind, value in (("info:implementationSource", "token", '"sourceAsset"'),
                                          ("info:sourceAsset", "asset", "@lunco://models/tests/ConnectionWorkspaceProbe.mo@"),
                                          ("lunco:program:realtimeSafe", "bool", "true"),
                                          ("inputs:feedback", "double", "0"),
                                          ("outputs:signal", "double", "0")):
                    ops.append({"SetAttribute": {"edit_target": "@root@", "path": path,
                                                "name": attr, "type_name": kind, "value": value}})
            ops.append({"SetConnection": {"edit_target": "@root@", "path": "/World/Source", "name": "inputs:feedback",
                                          "type_name": "double", "sources": ["/World/Source.outputs:signal"]}})
            cmd("ApplyUsdOps", doc_id=doc, ops=ops, label="Author connection workspace fixture")
            cmd("FocusPanel", id="usd_connection_canvas")
            cmd("SetConnectionSource", active_scene=False)
            wait_for(session, lambda: diagram().get("editing") and diagram().get("connection_count") == 1, "document topology")
            wait_for(session, lambda: diagram().get("hidden_local_connection_count") == 1, "authored presentation policy")
            wait_for(session, lambda: any(n.get("has_modelica_icon") for n in diagram()["nodes"]), "source-owned Modelica icon")
            gate('connection_workspace_view("topology", 1);')
            gate("connection_workspace_preview();")
            gate("connection_workspace_icon();")
            selector = {"edit_target": "@root@", "path": "/World/Source",
                        "name": "info:sourceAsset:subIdentifier", "type_name": "token"}
            cmd("ApplyUsdOp", doc_id=doc, op={"SetAttribute": {**selector, "value": '"MissingIconClass"'}})
            wait_for(session, lambda: any(n["path"] == "/World/Source" and not n["has_modelica_icon"]
                                          for n in diagram()["nodes"]), "retired icon after class selection changes")
            cmd("UndoDocument", doc_id=doc)
            wait_for(session, lambda: any(n["path"] == "/World/Source" and n["has_modelica_icon"]
                                          for n in diagram()["nodes"]), "restored authored icon")
            gate("connection_workspace_icon();")
            cmd("FrameConnectionDiagram", view_id=diagram()["view_id"])
            capture("topology.png")

            def port(path, name):
                state = diagram()
                node = next(n for n in state["nodes"] if n["path"] == path)
                return next(p for p in node["ports"] if p["name"] == name)

            def drag(source, target):
                cmd("InjectWindowInput", event={"Focus": {"focused": True}})
                x, y = source["screen_x"], source["screen_y"]
                cmd("InjectWindowInput", event={"PointerMove": {"x": x, "y": y}})
                cmd("InjectWindowInput", event={"PointerButton": {"button": "Primary", "state": "Pressed", "x": x, "y": y}})
                for i in range(1, 9):
                    x = source["screen_x"] + (target["screen_x"] - source["screen_x"]) * i / 8
                    y = source["screen_y"] + (target["screen_y"] - source["screen_y"]) * i / 8
                    cmd("InjectWindowInput", event={"PointerMove": {"x": x, "y": y}})
                    time.sleep(.04)
                cmd("InjectWindowInput", event={"PointerButton": {"button": "Primary", "state": "Released", "x": x, "y": y}})

            drag(port("/World/Source", "outputs:signal"), port("/World/Sink", "inputs:feedback"))
            wait_for(session, lambda: diagram()["connection_count"] == 2, "native port connection")
            gate("connection_workspace_binding_count(2);")
            session.capture_screenshot(area / "native-connected.png")

            def route(state):
                return next(link["route"] for link in state["connections"]
                            if link["source"] == "/World/Source" and link["target"] == "/World/Sink")

            cmd("FrameConnectionDiagram", view_id=diagram()["view_id"])
            panel = next(p for p in cmd("InspectWorkbench")["rendered_panels"]
                         if p["id"] == "usd_connection_canvas")

            def framed_sink():
                current = diagram()
                node = next(n for n in current["nodes"] if n["path"] == "/World/Sink")
                return current if panel["left"] + 30 < node["screen_x"] < panel["left"] + panel["width"] - 30 else None

            before = wait_for(session, framed_sink, "fitted drag target")
            card = next(node for node in before["nodes"] if node["path"] == "/World/Sink")
            x, y = card["screen_x"], card["screen_y"]
            direction = 1 if y < height / 2 else -1
            dx = -20
            dy = direction * 27
            cmd("InjectWindowInput", event={"PointerMove": {"x": x, "y": y}})
            cmd("InjectWindowInput", event={"PointerButton": {"button": "Primary", "state": "Pressed", "x": x, "y": y}})
            for fraction in (.25, .5, .75, 1.0):
                cmd("InjectWindowInput", event={"PointerMove": {"x": x + dx * fraction, "y": y + dy * fraction}})
                time.sleep(.05)
            try:
                initial_route = route(before)
                session.capture_screenshot(area / "node-drag-held.png")
                gate(f"connection_workspace_drag_route({initial_route[1]['x']!r}, {initial_route[-2]['y']!r});")
                cmd("InjectWindowInput", event={"Key": {"key": "Escape", "state": "Pressed", "repeat": False, "text": None}})
                cmd("InjectWindowInput", event={"Key": {"key": "Escape", "state": "Released", "repeat": False, "text": None}})
                wait_for(session, lambda: route(diagram()) == route(before), "cancel restores the original wire route")
            finally:
                cmd("InjectWindowInput", event={"PointerButton": {"button": "Primary", "state": "Released", "x": x + dx, "y": y + dy}})
            if diagram()["definitions"] != before["definitions"]:
                raise RuntimeError("a cancelled node drag changed saved layout positions")

            drag(port("/World/Source", "inputs:feedback"), port("/World/Sink", "inputs:feedback"))
            gate("connection_workspace_rejection(2);")
            if not rejection(session, "SetConnectionWorkspace", view_id=diagram()["view_id"], presentation="missing"):
                raise RuntimeError("invalid workspace mode was admitted")
            gate("connection_workspace_rejection(2);")

            cmd("UndoDocument", doc_id=doc)
            wait_for(session, lambda: diagram()["connection_count"] == 1, "USD undo")
            gate("connection_workspace_binding_count(1);")
            cmd("RedoDocument", doc_id=doc)
            wait_for(session, lambda: diagram()["connection_count"] == 2, "USD redo")
            for mode in ("interface", "bindings"):
                workspace(mode)
                gate(f'connection_workspace_view("{mode}", 1);')
                capture(f"{mode}.png")
            def endpoint(path, port):
                return {"key": "/World/" + path, "port": port}

            def binding(source, target):
                return {"from": source, "to": target}

            source = endpoint("Source", "outputs:signal")
            sink = endpoint("Sink", "inputs:feedback")
            sink_output = endpoint("Sink", "outputs:signal")
            alternative = endpoint("Alternative", "outputs:signal")
            alternative_input = endpoint("Alternative", "inputs:feedback")
            original = binding(source, sink)
            local_pair = ["/World/Source.outputs:signal", "/World/Source.inputs:feedback"]
            original_pair = ["/World/Source.outputs:signal", "/World/Sink.inputs:feedback"]
            backup_pair = ["/World/Alternative.outputs:signal", "/World/Sink.inputs:feedback"]

            def exact(expected):
                def matches():
                    actual = [[link["source"] + "." + link["source_port"], link["target"] + "." + link["target_port"]] for link in diagram()["connections"]]
                    return sorted(actual) == sorted(expected)
                wait_for(session, matches, "exact edited topology")
                gate("connection_workspace_exact_bindings(" + json.dumps(expected) + ");")

            def apply_binding(new_source, new_target, previous=None, replace=False):
                current = diagram()
                return cmd("ApplyConnectionBinding", view_id=current["view_id"], parent_gen=current["source_generation"],
                           **binding(new_source, new_target), previous=previous, replace_sources=replace)

            cmd("ApplyUsdOps", doc_id=doc, label="Author binding edit acceptance", ops=[
                {"AddPrim": {"edit_target": "@root@", "parent_path": "/World", "name": "Alternative", "type_name": "Scope", "reference": None, "reference_prim_path": None}},
                *[{"SetAttribute": {"edit_target": "@root@", "path": "/World/Alternative", "name": name, "type_name": "double", "value": "0"}} for name in ("inputs:feedback", "outputs:signal")],
            ])
            wait_for(session, lambda: any(n["path"] == "/World/Alternative" for n in diagram()["nodes"]), "alternative endpoint")
            old_generation = diagram()["source_generation"]
            apply_binding(alternative, sink)
            base = [local_pair, original_pair, backup_pair]
            exact(base)
            stale = rejection(session, "ApplyConnectionBinding", view_id=diagram()["view_id"], parent_gen=old_generation,
                              **binding(sink_output, sink), previous=original, replace_sources=False)
            if "USD source changed" not in stale:
                raise RuntimeError("stale binding edit did not return its generation diagnostic: " + stale)
            exact(base)
            missing = rejection(session, "ApplyConnectionBinding", view_id=diagram()["view_id"], parent_gen=diagram()["source_generation"],
                                **binding(source, sink), previous=binding(source, alternative_input), replace_sources=False)
            if "original binding is no longer present" not in missing:
                raise RuntimeError("missing original binding did not return its diagnostic: " + missing)
            exact(base)
            apply_binding(source, alternative_input, previous=original)
            exact([local_pair, backup_pair, ["/World/Source.outputs:signal", "/World/Alternative.inputs:feedback"]])
            cmd("UndoDocument", doc_id=doc)
            exact(base)
            apply_binding(sink_output, sink, previous=original)
            edited = [local_pair, backup_pair, ["/World/Sink.outputs:signal", "/World/Sink.inputs:feedback"]]
            exact(edited)
            cmd("UndoDocument", doc_id=doc)
            exact(base)
            cmd("RedoDocument", doc_id=doc)
            exact(edited)
            cmd("UndoDocument", doc_id=doc)
            exact(base)
            apply_binding(sink_output, sink, replace=True)
            exact([local_pair, ["/World/Sink.outputs:signal", "/World/Sink.inputs:feedback"]])
            cmd("UndoDocument", doc_id=doc)
            exact(base)
            apply_binding(source, sink, previous=binding(alternative, sink))
            exact([local_pair, original_pair])
            cmd("ApplyUsdOp", doc_id=doc, op={"RemovePrim": {"edit_target": "@root@", "path": "/World/Alternative"}})
            wait_for(session, lambda: not any(n["path"] == "/World/Alternative" for n in diagram()["nodes"]), "alternative endpoint retirement")
            view_id = diagram()["view_id"]
            cmd("SetConnectionGroup", view_id=view_id, group_id="workspace-probes", label="Probe pair", members=["/World/Source", "/World/Sink"])
            wait_for(session, lambda: any(g["id"] == "workspace-probes" for g in diagram()["groups"]), "manual view group")
            cmd("SetConnectionGroupCollapsed", view_id=view_id, group_id="workspace-probes", collapsed=True)
            wait_for(session, lambda: diagram()["rendered_connection_count"] == 0, "group collapse")
            gate("connection_workspace_grouped(0);")
            cmd("SetConnectionGroupCollapsed", view_id=view_id, group_id="workspace-probes", collapsed=False)
            wait_for(session, lambda: diagram()["rendered_connection_count"] == 1, "group expansion")
            gate("connection_workspace_grouped(1);")
            dense = [{"AddPrim": {"edit_target": "@root@", "parent_path": "/World", "name": "Dense", "type_name": "Scope", "reference": None, "reference_prim_path": None}}]
            for index in range(40):
                leaf = f"long_engineering_interface_name_with_units_and_reference_frame_{index:02d}"
                for prefix in ("inputs:", "outputs:"):
                    dense.append({"SetAttribute": {"edit_target": "@root@", "path": "/World/Dense", "name": prefix + leaf, "type_name": "double", "value": "0"}})
                dense.append({"SetConnection": {"edit_target": "@root@", "path": "/World/Dense", "name": "inputs:" + leaf, "type_name": "double", "sources": ["/World/Dense.outputs:" + leaf]}})
            cmd("ApplyUsdOps", doc_id=doc, ops=dense, label="Author dense interface acceptance")
            wait_for(session, lambda: diagram()["connection_count"] == 42, "dense topology")
            cmd("SelectConnectionElement", view_id=diagram()["view_id"], key="/World/Dense", reveal=True)
            workspace("interface")
            gate("connection_workspace_dense();")
            capture("dense-interface.png")
            workspace("bindings")
            capture("dense-bindings.png")
            panel = next(p for p in cmd("InspectWorkbench")["rendered_panels"] if p["id"] == "usd_connection_canvas")
            cmd("InjectWindowInput", event={"PointerMove": {"x": panel["left"] + panel["width"] / 2, "y": panel["top"] + panel["height"] - 25}})
            for step in range(4):
                cmd("InjectWindowInput", event={"Scroll": {"x": 0.0, "y": -250.0}})
                time.sleep(.08)
            capture("dense-bindings-scrolled.png")
            gate("connection_workspace_dense();")
            cmd("ApplyUsdOp", doc_id=doc, op={"RemovePrim": {"edit_target": "@root@", "path": "/World/Dense"}})
            wait_for(session, lambda: diagram()["connection_count"] == 2, "dense fixture retirement")
            source = twin_root / "workspace.usda"
            cmd("SaveAsDocument", doc_id=doc, path=str(source))
            wait_for(session, source.is_file, "USD save")
            view_id = diagram()["view_id"]
            cmd("OpenConnectionNode", view_id=view_id, key="/World/Source", program_path="/World/Source")
            model_doc = wait_for(session, lambda: next((d["doc_id"] for d in cmd("ListOpenDocuments")["open_documents"] if d["kind"] == "modelica"), None), "Modelica editor")
            original = cmd("GetDocumentSource", doc_id=model_doc)["source"]
            cmd("CloseDocument", doc_id=model_doc)
            cmd("CreateScratchModelicaDocument", source=original, name="ConnectionWorkspaceProbe")
            copy_doc = wait_for(session, lambda: next((d["doc_id"] for d in cmd("ListOpenDocuments")["open_documents"] if d["kind"] == "modelica"), None), "editable model copy")
            cmd("SetDocumentSource", doc_id=copy_doc, source=original.replace("2 + sin(time)", "2.5 + sin(time)"))
            gate(f'connection_workspace_model_source({copy_doc}, "2.5 + sin(time)");')
            cmd("UndoDocument", doc_id=copy_doc)
            gate(f'connection_workspace_model_source({copy_doc}, "2 + sin(time)");')
            cmd("RedoDocument", doc_id=copy_doc)
            gate(f'connection_workspace_model_source({copy_doc}, "2.5 + sin(time)");')
            model_copy = area / "ConnectionWorkspaceProbe.mo"
            cmd("SaveAsDocument", doc_id=copy_doc, path=str(model_copy))
            wait_for(session, model_copy.is_file, "Modelica save")
            capture("modelica-editor.png")
            cmd("CloseDocument", doc_id=copy_doc)
            cmd("CreateScratchModelicaDocument", source=original.replace("ConnectionWorkspaceProbe", "TwinConnectionProbe"), name="TwinConnectionProbe")
            twin_model = wait_for(session, lambda: next((d["doc_id"] for d in cmd("ListOpenDocuments")["open_documents"] if d["kind"] == "modelica"), None), "Twin model source")
            cmd("SaveAsDocument", doc_id=twin_model, path=str(twin_root / "TwinConnectionProbe.mo"))
            wait_for(session, (twin_root / "TwinConnectionProbe.mo").is_file, "Twin model save")
            cmd("CloseDocument", doc_id=twin_model)
            (twin_root / "twin.toml").write_text('name = "Connection workspace acceptance"\nversion = "0.1.0"\n[usd]\ndefault_scene = "workspace.usda"\n')
            cmd("OpenTwin", path=str(twin_root))
            twin = wait_for(session, lambda: cmd("InspectWorkbenchNavigation").get("twin_id"), "owned Twin")
            cmd("OpenTwinScene", twin_id=twin, relative_path="workspace.usda")
            wait_for(session, lambda: cmd("GetReadiness")["ready"] and not cmd("GetReadiness")["world_hold"], "live fixture")
            cmd("ActivatePerspective", id="systems")
            cmd("SetConnectionSource", active_scene=True)
            cmd("FocusPanel", id="usd_connection_canvas")
            wait_for(session, lambda: diagram().get("active_scene") and diagram().get("connection_count") == 2, "live diagram")
            live_source = diagram()["source"]
            cmd("OpenFile", path=live_source.rsplit("/", 1)[0] + "/TwinConnectionProbe.mo")
            twin_model = wait_for(session, lambda: next((d["doc_id"] for d in cmd("ListOpenDocuments")["open_documents"] if d["kind"] == "modelica"), None), "stable Twin Modelica source")
            gate(f'connection_workspace_model_source({twin_model}, "model TwinConnectionProbe");')
            cmd("CloseDocument", doc_id=twin_model)
            cmd("OpenUsdSourceDocument", source=live_source)
            cmd("ActivatePerspective", id="editor")
            cmd("FocusPanel", id="usd_connection_canvas")
            cmd("SetConnectionSource", active_scene=False)
            wait_for(session, lambda: diagram().get("editing") and diagram().get("connection_count") == 2, "live source document authoring")
            gate("connection_workspace_preview();")
            if not rejection(session, "OpenUsdSourceDocument", source="twin://connection-workspace-unmounted/workspace.usda"):
                raise RuntimeError("unmounted source was opened")
            gate("connection_workspace_binding_count(2);")
            cmd("SetConnectionSource", active_scene=True)
            wait_for(session, lambda: diagram().get("active_scene"), "restored live source")
            current = diagram()
            readonly = rejection(session, "ApplyConnectionBinding", view_id=current["view_id"], parent_gen=current["source_generation"],
                                 **binding(endpoint("Source", "outputs:signal"), endpoint("Sink", "inputs:feedback")),
                                 previous=None, replace_sources=False)
            if "Choose Edit connections" not in readonly:
                raise RuntimeError("live scene binding edit did not reject at its document owner: " + readonly)
            gate("connection_workspace_rejection(2);")
            workspace("bindings", live_values=True, animate_changes=True)
            wait_for(session, lambda: all(link["status"] == "bound" for link in diagram()["connections"]), "owner-bound connections")
            cmd("SetTimeTransport", playing=True)
            wait_for(session, lambda: all(link.get("source_value") is not None for link in diagram()["connections"]), "live sampled values")
            first = diagram()
            time.sleep(.4)
            previous = [link["source_value"] for link in first["connections"]]
            gate("connection_workspace_live(" + json.dumps(previous) + ");")
            session.capture_screenshot(area / "live-bindings.png")
            sink = int(cmd("RunRhai", code='print(find("/World/Sink"));')["stdout"].strip())
            cmd("SetPorts", target=sink, writes=[["feedback", 2.75]], producer_id=4587)
            wait_for(session, lambda: any(link["target"] == "/World/Sink" and link["status"] == "held" and link["target_value"] == 2.75 for link in diagram()["connections"]), "held input status")
            gate("connection_workspace_held();")
            cmd("ReleasePort", target=sink, name="feedback", producer_id=4587)
            wait_for(session, lambda: all(link["status"] == "bound" for link in diagram()["connections"]), "released input binding")
            cmd("SetTimeTransport", playing=False)
            wait_for(session, lambda: not any(link["animating"] for link in diagram()["connections"]), "paused highlight")
            gate("connection_workspace_paused();")
            (area / "diagram.json").write_text(json.dumps(diagram(), indent=2))
            cmd("ClearScene")
            wait_for(session, lambda: not diagram().get("built"), "scene-derived workspace retirement")
        finally:
            (area / "last-diagram.json").write_text(json.dumps(diagram(), indent=2))
            session.capture_screenshot(area / "last-window.png")
            for opened in cmd("ListOpenDocuments")["open_documents"]:
                cmd("CloseDocument", doc_id=opened["doc_id"])
            wait_for(session, lambda: not cmd("ListOpenDocuments")["open_documents"], "owned document retirement")
    assert_render_log_clean(log)
    print(f"CONNECTION WORKSPACE: PASS; evidence={area}", flush=True)


if __name__ == "__main__":
    main()
