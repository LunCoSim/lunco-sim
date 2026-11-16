#!/usr/bin/env python3
"""Exercise terminal editor results, file preservation, and rapid GPU shutdown."""
import argparse
import json
import os
from pathlib import Path
import uuid

from runtime import ROOT, BINARY, ProductionSession, assert_render_log_clean
from test_scene_loading_settings import execute, wait_for


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    args = parser.parse_args()
    area = ROOT / "target" / ("ui-review-followup-" + uuid.uuid4().hex)
    config = area / "config"
    config.mkdir(parents=True)
    (config / "settings.json").write_text(json.dumps({"window": {
        "width": 1280, "height": 800, "x": None, "y": None, "maximized": False,
    }}))
    os.environ["LUNCOSIM_CONFIG"] = str(config)
    os.environ.pop("LUNCOSIM_EPHEMERAL_SETTINGS", None)
    print(f"Evidence: {area}", flush=True)
    results = {}
    for cycle in range(3):
        log = area / f"runtime-{cycle}.log"
        with ProductionSession(args.port, windowed=True, extra_args=("--no-throttle",), log_path=log) as session:
            pid = session.process.pid
            if Path(f"/proc/{pid}/cwd").resolve() != ROOT or Path(f"/proc/{pid}/exe").resolve() != BINARY.resolve():
                raise RuntimeError("runtime does not belong to this checkout")
            print(f"OWNED pid={pid} port={args.port} cycle={cycle}", flush=True)

            def command(command_name, **params):
                return execute(session, command_name, **params)

            def rhai(code):
                return command("RunRhai", code=code)["stdout"].strip()

            def gate(call):
                output = rhai((ROOT / "assets/scripting/tests/test_ui_review.rhai").read_text() + "\n" + call)
                if "TESTS_FAIL" in output or ": PASS" not in output:
                    raise RuntimeError(output)
                print(output, flush=True)

            if cycle == 0:
                twin = area / "shader-twin"
                command("CreateTwin", path=str(twin))
                owner = wait_for(session, lambda: command("InspectWorkbenchNavigation").get("twin_id"), "Twin")
                gate(f'ui_twin_collision({json.dumps(str(twin))}, {owner});')
                # This identity is deliberately absent from ApiEntityRegistry.
                gate(f'ui_shader_rejections({json.dumps(str(area / "absent.wesl"))}, 9007199254740000);')
                if any((twin / "shaders" / f"{name}.wesl").exists() for name in ("invalid_probe", "invalid_target")):
                    raise RuntimeError("rejected creation wrote a shader")
                source_uri = f'twin://{rhai("print(twin_source_identity());")}/shaders/protected.wesl'
                created = command("CreateShader", name="protected", template="solid")
                if created.get("source_uri") != source_uri:
                    raise RuntimeError(f"creation must return its stable source URI: {created}")
                shader = twin / "shaders/protected.wesl"
                original = shader.read_bytes()
                imported = area / "incoming.wesl"
                imported.write_bytes(original + b"\n// independent import\n")
                gate(f'ui_shader_collisions({json.dumps(str(imported))});')
                if shader.read_bytes() != original:
                    raise RuntimeError("failed creation/import replaced the existing shader")
                installed = command("ImportShader", source_path=str(imported), name="imported")
                if installed.get("source_uri") != source_uri.replace("protected.wesl", "imported.wesl"):
                    raise RuntimeError(f"import must return its stable source URI: {installed}")
                if (twin / "shaders/imported.wesl").read_bytes() != imported.read_bytes():
                    raise RuntimeError("successful import did not preserve the supplied source")
                results["shaders"] = {"created": created, "imported": installed, "original_preserved": True}
            else:
                command("CreateTwin", path=str(area / f"switch-twin-{cycle}"))
                wait_for(session, lambda: command("InspectWorkbenchNavigation").get("twin_id"), "switch Twin")
                command("CreateShader", name="switch_probe", template="solid")
                command("ImportShader", source_path=str(area / "incoming.wesl"), name="switch_import")

            previous = command("InspectWorkbenchNavigation").get("twin_id")
            command("OpenTwin", path=str(ROOT / "assets/tutorials/classroom"))
            owner = wait_for(session, lambda: (x if (x := command("InspectWorkbenchNavigation").get("twin_id")) not in (None, previous) else None), "classroom")
            command("OpenTwinScene", twin_id=owner, relative_path="scene.usda")
            target = wait_for(session, lambda: (x if (x := int(rhai('print(find("/RoverLab/Guide"));'))) >= 0 else None), "guide")
            try:
                wait_for(session, lambda: command("GetReadiness")["ready"] and not command("GetReadiness")["world_hold"], "ready")
            except RuntimeError:
                (area / f"readiness-failure-{cycle}.json").write_text(json.dumps({
                    "readiness": command("GetReadiness"),
                    "diagnostics": command("GetDiagnostics"),
                    "script": command("ScriptStatus", target=target),
                }, indent=2))
                raise
            command("RunScenario", target=target, source="fn on_start(me, ctx) {}")
            wait_for(session, lambda: command("ScriptStatus", target=target)["state"] == "ready", "script")
            if cycle == 0:
                saved = command("SaveScenario", target=target)
                if not saved.get("doc_id") or saved.get("generation") is None:
                    raise RuntimeError(f"SaveScenario must return the completed USD acknowledgement: {saved}")
                gate("ui_document_applied();")
                results["save_scenario"] = saved
                document = next(d["doc_id"] for d in command("ListOpenDocuments")["open_documents"] if d["kind"] == "usd" and d["dirty"])
                command("SaveAsDocument", doc_id=document, path=str(area / "saved-scene.usda"))
                (area / "schema.json").write_text(json.dumps(session.post({"type": "DiscoverSchema"}), indent=2))
            # Exit immediately after admission; no artificial render warmup.
        assert_render_log_clean(log)
        print(f"PASS: rapid render shutdown {cycle + 1}, exit=0 and port released", flush=True)
    (area / "results.json").write_text(json.dumps(results, indent=2))


if __name__ == "__main__":
    main()
