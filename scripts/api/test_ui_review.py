#!/usr/bin/env python3
"""Exercise editor drafts and compact layouts in an owned production window."""
import argparse
import json
import os
from pathlib import Path
import time
import uuid

from runtime import ROOT, BINARY, ProductionSession, assert_render_log_clean, get_json
from test_scene_loading_settings import execute, wait_for

ASSERTIONS = ROOT / "assets/scripting/tests/test_ui_review.rhai"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--compact", action="store_true")
    args = parser.parse_args()
    area = ROOT / "target" / ("ui-review-" + uuid.uuid4().hex)
    config = area / "config"
    config.mkdir(parents=True)
    width, height = (960, 540) if args.compact else (1280, 800)
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
            raise RuntimeError("owned runtime does not match the checkout")
        print(f"OWNED pid={pid} port={args.port}", flush=True)

        def command(name, **params):
            return execute(session, name, **params)

        def rhai(code):
            return command("RunRhai", code=code)["stdout"]

        def gate(function):
            output = rhai(ASSERTIONS.read_text() + "\n" + function)
            if "TESTS_FAIL" in output or ": PASS" not in output:
                raise RuntimeError(output)
            print(output, flush=True)

        def key(name, state="Pressed", text=None):
            command("InjectWindowInput", event={"Key": {
                "key": name, "state": state, "text": text, "repeat": False,
            }})

        def click(x, y):
            command("InjectWindowInput", event={"PointerMove": {"x": x, "y": y}})
            for state in ("Pressed", "Released"):
                command("InjectWindowInput", event={"PointerButton": {
                    "button": "Primary", "state": state, "x": x, "y": y,
                }})
                time.sleep(.06)

        def panel(name):
            return wait_for(session, lambda: next((p for p in command("InspectWorkbench")["rendered_panels"]
                                                   if p["id"] == name), None), name)

        def replace_source(text):
            rect = panel("rhai_editor")
            session.capture_screenshot(area / "before-input.png")
            click(rect["left"] + 40, rect["top"] + 135)
            key("ControlLeft"); key("KeyA"); key("KeyA", "Released"); key("ControlLeft", "Released")
            key("KeyX", text=text); key("KeyX", "Released")
            wait_for(session, lambda: command("InspectBehaviourEditor")["source"] == text, "typed draft")

        def run_draft():
            rect = panel("rhai_editor")
            click(rect["left"] + 30, rect["top"] + 10)

        command("ActivatePerspective", id="editor")
        command("ResetWorkspaceLayout")
        for name in ("sysml_requirements", "sysml_requirement_details"):
            command("FocusPanel", id=name)
            panel(name)
            gate(f'ui_panel_visible("{name}");')
        gate("ui_titles_unique();")
        session.capture_screenshot(area / "empty-editor.png")
        command("OpenTwin", path=str(ROOT / "assets/tutorials/classroom"))
        twin = wait_for(session, lambda: command("InspectWorkbenchNavigation").get("twin_id"), "Twin admission")
        command("OpenTwinScene", twin_id=twin, relative_path="scene.usda")
        wait_for(session, lambda: command("GetReadiness")["ready"] and not command("GetReadiness")["world_hold"], "scene admission")
        target = wait_for(session, lambda: (value if (value := int(rhai('print(find("/RoverLab/Guide"));').strip())) >= 0 else None), "projected guide")
        other = int(rhai('print(find("/RoverLab/Rover"));').strip())
        command("RunScenario", target=target, source="fn on_start(me, ctx) {}")
        command("ClearGuidedOverlay")
        for preview in command("InspectUsdViewport")["previews"]:
            command("CloseUsdPreview", preview=preview["preview"])
        command("ActivatePerspective", id="rover_build")
        command("FocusPanel", id="rhai_editor")
        command("SelectEntity", entity_id=target)
        wait_for(session, lambda: command("InspectBehaviourEditor")["selected_document"], "behaviour source")
        invalid = "fn on_start(me, ctx) { let UI_DRAFT_SENTINEL = ; }"
        replace_source(invalid)
        command("SelectEntity", entity_id=other)
        command("SelectEntity", entity_id=target)
        gate("ui_draft_retained();")
        run_draft()
        wait_for(session, lambda: any("Unexpected" in d["message"] for d in command("ScriptStatus", target=target)["diagnostics"]), "invalid draft diagnostic")
        gate("ui_draft_rejected();")
        session.capture_screenshot(area / "failed-draft.png")
        replace_source("fn on_start(me, ctx) { this.UI_DRAFT_SENTINEL = true; }")
        run_draft()
        wait_for(session, lambda: not command("InspectBehaviourEditor")["dirty"] and not command("InspectBehaviourEditor")["pending"], "valid compile")
        gate("ui_valid_draft();")
        gate(f"ui_save_rejected({other});")
        script_doc = command("InspectBehaviourEditor")["selected_document"]
        command("FocusPanel", id="rover_code")
        rect = panel("rover_code")
        session.capture_screenshot(area / "source-viewer.png")
        click(rect["left"] + 20, rect["top"] + 160)
        key("KeyX", text="FORBIDDEN_EDIT"); key("KeyX", "Released")
        gate(f"ui_source_read_only({script_doc});")
        command("FocusPanel", id="rhai_editor")
        rect = panel("rhai_editor")
        click(rect["left"] + 110, rect["top"] + 35)
        wait_for(session, lambda: any(d["kind"] == "usd" and d["dirty"] for d in command("ListOpenDocuments")["open_documents"]), "document authoring")
        gate("ui_document_applied();")
        doc = next(d["doc_id"] for d in command("ListOpenDocuments")["open_documents"] if d["kind"] == "usd")
        command("SaveAsDocument", doc_id=doc, path=str(area / "saved-scene.usda"))
        wait_for(session, lambda: (area / "saved-scene.usda").exists(), "saved document")
        command("ClearScene")
        wait_for(session, lambda: command("InspectBehaviourEditor")["draft_count"] == 0, "draft retirement")
        gate("ui_drafts_retired();")
        command("ActivatePerspective", id="modelica_analyze")
        command("OpenFile", path=str(ROOT / "assets/models/LunCo/Electrical/Battery.mo"))
        wait_for(session, lambda: any(p["id"] == "modelica_model_view" for p in command("InspectWorkbench")["rendered_panels"]), "model view")
        command("ClearGuidedOverlay")
        time.sleep(8)  # Let transient verdict notifications retire before visual inspection.
        session.capture_screenshot(area / "battery.png")
        command("ActivatePerspective", id="input_settings")
        command("ResetWorkspaceLayout")
        command("FocusPanel", id="input_keybindings")
        panel("input_keybindings")
        time.sleep(.25)
        session.capture_screenshot(area / "keybindings.png")
        # This driver fixes window dimensions and exercises the measured native
        # first-row controls; assertions read the shared input owner through Rhai.
        click(80, 128)
        key("KeyX", text="action"); key("KeyX", "Released")
        time.sleep(.2)
        click(358, 180)
        time.sleep(.1)
        key("F8"); key("F8", "Released")
        time.sleep(.2)
        click(405, 202)
        wait_for(session, lambda: next(row for row in command("InspectInputBindings")["global_intents"] if row["name"] == "action")["keys"] == ["F8"], "recorded action")
        gate('ui_intent_keys("action", ["F8"]);')
        click(150, 180)
        wait_for(session, lambda: not next(row for row in command("InspectInputBindings")["global_intents"] if row["name"] == "action")["keys"], "unbound action")
        gate('ui_intent_keys("action", []);')
        click(280, 180)
        wait_for(session, lambda: next(row for row in command("InspectInputBindings")["global_intents"] if row["name"] == "action")["keys"] == ["KeyF"], "default action")
        gate('ui_intent_keys("action", ["KeyF"]);')
        gate("ui_camera_empty_state();")
        (area / "schema.json").write_text(json.dumps(get_json(args.port, "/api/commands/schema"), indent=2))
        gate("ui_export_rejected();")
        export = area / "snapshot.csv"
        content = "time (s),a,b\n0,1,\n1,,2\n"
        command("ExportText", suggested_name="snapshot.csv", content=content, path=str(export))
        wait_for(session, lambda: export.exists(), "export completion")
        if export.read_text() != content:
            raise RuntimeError("export snapshot bytes changed")
        previous_twin = command("InspectWorkbenchNavigation").get("twin_id")
        command("OpenTwin", path=str(ROOT / "assets/scenes/tests/sysml_async_analysis"))
        wait_for(session, lambda: command("InspectWorkbenchNavigation").get("twin_id") not in (None, previous_twin), "requirements Twin")
        source_identity = rhai("print(twin_source_identity());").strip()
        wait_for(session, lambda: command("AnalyzeSysml", path="twin://" + source_identity, tables=["requirements"]).get("ok"), "requirements analysis")
        command("ActivatePerspective", id="editor")
        command("ResetWorkspaceLayout")
        for name in ("sysml_requirements", "sysml_requirement_details"):
            command("FocusPanel", id=name)
            panel(name)
            gate(f'ui_panel_visible("{name}");')
        time.sleep(8)
        session.capture_screenshot(area / "requirements.png")
        # The source action is in the fixed first detail block; constrain its
        # pointer to this panel so the same gesture works at both widths.
        details = panel("sysml_requirement_details")
        click(details["left"] + (75 if args.compact else 205), details["top"] + (230 if args.compact else 170))
        wait_for(session, lambda: any(document["kind"] == "sysml" for document in command("ListOpenDocuments")["open_documents"]), "requirement source document")
        gate("ui_sysml_source_open();")
        time.sleep(.4)  # Capture the fully opened source window after egui fade-in.
        session.capture_screenshot(area / "requirement-source.png")
        (area / "requirements-layout.json").write_text(json.dumps(command("InspectWorkbench"), indent=2))
        # A large source set exercises virtualized cards and actual wheel overflow.
        # SysML is plain text; authored USD is always written through document APIs.
        bulk = area / "bulk"
        bulk.mkdir()
        (bulk / "requirements.sysml").write_text("package Scrolling {\nrequirement def DisplayRequirement { doc /* A scrolling requirement preview. */ }\n" +
            "\n".join(f"requirement req{index:03} : DisplayRequirement;" for index in range(100)) + "\n}\n")
        previous = command("InspectWorkbenchNavigation")["twin_id"]
        command("CreateTwin", path=str(bulk))
        wait_for(session, lambda: command("InspectWorkbenchNavigation").get("twin_id") not in (None, previous), "large requirements Twin")
        source_identity = rhai("print(twin_source_identity());").strip()
        wait_for(session, lambda: command("AnalyzeSysml", path="twin://" + source_identity, tables=["requirements"]).get("ok"), "large requirements analysis")
        command("FocusPanel", id="sysml_requirements")
        gate("ui_sysml_large_inventory();")
        rect = panel("sysml_requirements")
        time.sleep(.3)
        session.capture_screenshot(area / "requirements-many-start.png")
        command("InjectWindowInput", event={"PointerMove": {"x": rect["left"] + 70, "y": rect["top"] + (360 if args.compact else 500)}})
        command("InjectWindowInput", event={"Scroll": {"x": 0.0, "y": -1000.0}})
        time.sleep(.5)
        session.capture_screenshot(area / "requirements-many-end.png")
        assert_render_log_clean(log)
    print("PASS: owned process exited and API port released", flush=True)


if __name__ == "__main__":
    main()
