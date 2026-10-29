#!/usr/bin/env python3
"""Verify the classroom workshop and saved report in an owned windowed session."""
from __future__ import annotations

import argparse
import json
import time
from pathlib import Path

from runtime import BINARY, ROOT, ProductionSession, get_json, wait_for_authored_verdict


def command(session: ProductionSession, command_name: str, **params: object) -> dict:
    response = session.post({"type": "ExecuteCommand", "command": command_name, "params": params})
    if response.get("error"):
        raise RuntimeError(f"{command_name}: {response}")
    return response.get("data", {})


def wait_until(session: ProductionSession, description: str, predicate, timeout: float = 180):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = predicate()
        if result:
            return result
        readiness = command(session, "GetReadiness")
        if readiness.get("faulted"):
            raise RuntimeError(f"{description}: {readiness}")
        if session.process.poll() is not None:
            raise RuntimeError(f"owned process exited while waiting for {description}")
        time.sleep(0.25)
    raise RuntimeError(f"timed out waiting for {description}; log={session.log_path}")


def lab_card(session: ProductionSession) -> dict:
    return next((card for card in command(session, "InspectGuidedHud").get("cards", [])
                 if card.get("hud") == "lunar_rover_lab"), {})


def report(session: ProductionSession) -> dict:
    owner = command(session, "RunRhai", code='print(assembly_edit::document_for_entity(find_path("/RoverLab/Rover")));')
    document = int(owner["stdout"].strip())
    return command(session, "QueryUsdPrim", doc_id=document, path="/RoverLab/Results/Report", attrs=[
        "inputs:baseline_speed_mps", "inputs:changed_speed_mps", "inputs:baseline_mass_kg",
        "inputs:changed_mass_kg", "inputs:prediction_code", "inputs:practice"])


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    output = (args.output or ROOT / "target" / "classroom-onboarding" / str(time.time_ns())).resolve()
    output.mkdir(parents=True, exist_ok=False)
    saved = output / "participant-twin"
    with ProductionSession(args.port, windowed=True,
                           extra_args=("--no-throttle", "--no-vsync", "--window-pos", "1280x720+0+0"),
                           log_path=output / "runtime.log") as session:
        pid = session.process.pid
        if Path(f"/proc/{pid}/cwd").resolve() != ROOT or Path(f"/proc/{pid}/exe").resolve() != BINARY.resolve():
            raise RuntimeError("session does not belong to this checkout and binary")
        print(f"OWNED pid={pid} port={args.port} output={output}", flush=True)
        (output / "schema.json").write_text(json.dumps(get_json(args.port, "/api/commands/schema"), indent=2) + "\n")
        session.capture_screenshot(output / "opening.png")
        command(session, "OpenTwinAsset", manifest_asset="lunco://tutorials/classroom/twin.toml")
        wait_until(session, "initial workshop card", lambda: lab_card(session).get("hint", "").startswith("1/6"))
        session.capture_screenshot(output / "workshop.png")
        # Use a separate existing host so the observer cannot replace the lesson.
        source = command(session, "RunRhai", code='print(find_path("/RoverLab/BuildPad"));')
        observer = int(source["stdout"].strip())
        offset = session.log_path.stat().st_size
        command(session, "RunScenarioAsset", target=observer,
                source_asset="lunco://scenarios/tests/tutorial_rover_lab.rhai", reload_policy="Restart")
        wait_for_authored_verdict(session, title="Lunar Rover Lab", check_count=11, offset=offset, timeout_s=240)
        before = report(session)
        document = command(session, "InspectUsdDocument", doc_id=before["doc_id"])
        if not document["dirty"] or "Twin is saved" in lab_card(session).get("hint", ""):
            raise RuntimeError("unsaved report was presented as saved")
        command(session, "ActivatePerspective", id="analysis")
        session.capture_screenshot(output / "speed-graph.png")
        command(session, "ActivatePerspective", id="rover_build")
        session.capture_screenshot(output / "placement.png")
        command(session, "SaveAsTwin", folder=str(saved))
        wait_until(session, "saved report", lambda: "your Twin is saved" in lab_card(session).get("hint", ""))
        after = report(session)
        if after["attrs"] != before["attrs"]:
            raise RuntimeError("Twin promotion changed the measured report")
        documents = command(session, "ListOpenDocuments")["open_documents"]
        entry = next(doc for doc in documents if doc["doc_id"] == after["doc_id"])
        if entry["dirty"] or not Path(entry["origin"]["path"]).is_relative_to(saved):
            raise RuntimeError(f"saved report has the wrong document identity: {entry}")
        if not (saved / "twin.toml").is_file():
            raise RuntimeError("Twin promotion did not persist its manifest")
        session.capture_screenshot(output / "saved.png")
        # Replace the Twin before reopening it: a retained old HUD cannot pass.
        command(session, "CreateTwin", path=str(output / "retirement-probe"), name="Retirement probe")
        wait_until(session, "replacement Twin", lambda: command(session, "ReadActiveTwinContract").get("name") == "Retirement probe")
        if lab_card(session):
            raise RuntimeError("workshop HUD survived retirement of its Twin")
        command(session, "OpenTwin", path=str(saved))
        wait_until(session, "reopened report", lambda: "your Twin is saved" in lab_card(session).get("hint", ""))
        if report(session)["attrs"] != before["attrs"]:
            raise RuntimeError("reopening the Twin changed the report values")
        reopened = report(session)
        placement = command(session, "QueryUsdPrim", doc_id=reopened["doc_id"],
                            path="/RoverLab/BuildBlock", attrs=["xformOp:translate"])
        if placement["attrs"]["xformOp:translate"] != [9.0, 0.5, 3.0]:
            raise RuntimeError("reopening the Twin lost the authored block placement")
        session.capture_screenshot(output / "reopened.png")
        # A skipped UI tour must remain closed under later Next commands.
        command(session, "RunScenarioAsset", source_asset="lunco://tutorials/luncosim/welcome.rhai", reload_policy="Restart")
        wait_until(session, "UI tour", lambda: command(session, "InspectGuidedHud").get("tour"))
        command(session, "GuidedSkip")
        wait_until(session, "closed skipped tour", lambda: command(session, "InspectGuidedHud").get("tour") is None)
        command(session, "GuidedNext")
        time.sleep(0.5)
        if command(session, "InspectGuidedHud").get("tour") is not None:
            raise RuntimeError("Next revived a skipped tour")
        command(session, "OpenTwinAsset", manifest_asset="lunco://tutorials/classroom/twin.toml")
        wait_until(session, "fresh workshop", lambda: lab_card(session).get("hint", "").startswith("1/6"))
        command(session, "RunRhai", code='tutorials::action("rover_lab:checkpoint_2");')
        wait_until(session, "instructor checkpoint", lambda: lab_card(session).get("hint", "").startswith("3/6"))
        if "unassessed" not in lab_card(session).get("hint", ""):
            raise RuntimeError("instructor checkpoint claimed earlier assessment")
        command(session, "RunRhai", code='tutorials::action("rover_lab:restart");')
        wait_until(session, "reset workshop", lambda: lab_card(session).get("hint", "").startswith("1/6"))
        session.capture_screenshot(output / "reset.png")
        (output / "report.json").write_text(json.dumps(before, indent=2) + "\n")
    print("Classroom onboarding: PASS; saved report reopened and owned API port released", flush=True)


if __name__ == "__main__":
    main()
