#!/usr/bin/env python3
"""Verify live Modelica input admission through the production API and Rhai."""

from __future__ import annotations

import json
import os
import shutil
import sys
import time
from pathlib import Path
from tempfile import TemporaryDirectory
from urllib.error import HTTPError
from urllib.request import Request, urlopen

from runtime import BINARY, ProductionSession, ROOT


SCENE_SOURCE = ROOT / "assets/scenes/tests/rocket_engine_observables.usda"
SCENARIO_SOURCE = ROOT / "assets/scenarios/tests/modelica_input_admission.rhai"
TIMEOUT_S = float(os.environ.get("MODELICA_INPUT_TIMEOUT", "240"))
PORT = int(os.environ.get("MODELICA_INPUT_API_PORT", "4731"))
PRODUCER_ID = 8182
INPUT_NAME = "throttle"
INPUT_VALUE = 0.375


def response_data(response: dict, operation: str) -> dict:
    if response.get("error"):
        raise RuntimeError(f"{operation} failed: {response}")
    data = response.get("data")
    if not isinstance(data, dict):
        raise RuntimeError(f"{operation} returned no data object: {response}")
    return data


def execute(session: ProductionSession, command: str, params: dict | None = None) -> dict:
    return response_data(
        session.post(
            {
                "type": "ExecuteCommand",
                "command": command,
                "params": params or {},
            }
        ),
        command,
    )


def execute_expected_rejection(
    session: ProductionSession, command: str, params: dict
) -> tuple[int, dict]:
    request = Request(
        f"http://127.0.0.1:{session.port}/api/commands",
        data=json.dumps(
            {"type": "ExecuteCommand", "command": command, "params": params}
        ).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urlopen(request, timeout=10) as response:
            return response.status, json.loads(response.read().decode("utf-8"))
    except HTTPError as error:
        return error.code, json.loads(error.read().decode("utf-8"))


def verify_owned_session(session: ProductionSession) -> int:
    if session.process is None or session.process.pid is None:
        raise RuntimeError("the production API session has no owned process")
    pid = session.process.pid
    executable = Path(f"/proc/{pid}/exe").resolve()
    working_directory = Path(f"/proc/{pid}/cwd").resolve()
    if executable != BINARY.resolve() or working_directory != ROOT.resolve():
        raise RuntimeError(
            f"API session ownership mismatch: pid={pid}, exe={executable}, cwd={working_directory}"
        )
    return pid


def wait_for_modelica_fixture(session: ProductionSession) -> int:
    deadline = time.monotonic() + TIMEOUT_S
    while time.monotonic() < deadline:
        entities = response_data(session.post({"type": "ListEntities"}), "ListEntities")[
            "entities"
        ]
        targets = [entity for entity in entities if entity.get("name") == "Engine"]
        for target in targets:
            ports_response = session.post(
                {
                    "type": "ExecuteCommand",
                    "command": "ReadPorts",
                    "params": {"api_id": target["api_id"]},
                }
            )
            if ports_response.get("error"):
                continue
            ports = (ports_response.get("data") or {}).get("ports") or []
            if any(
                port.get("name") == INPUT_NAME
                and port.get("direction") == "in"
                for port in ports
            ):
                return int(target["api_id"])
        time.sleep(0.25)
    raise RuntimeError(
        f"RocketEngineObservables did not expose a compiled Engine input within {TIMEOUT_S:g}s"
    )


def wait_for_verdict(log_path: Path, offset: int) -> str:
    deadline = time.monotonic() + TIMEOUT_S
    while time.monotonic() < deadline:
        try:
            with log_path.open("rb") as stream:
                stream.seek(offset)
                output = stream.read().decode("utf-8", errors="replace")
        except OSError:
            output = ""
        if "MODELICA_INPUT_ADMISSION" in output:
            if "TESTS_FAIL" in output:
                raise RuntimeError(f"Modelica input Rhai gate failed:\n{output}")
            if "TESTS_OK" in output:
                return output
        time.sleep(0.25)
    raise RuntimeError(f"Modelica input Rhai gate produced no verdict; log={log_path}")


def main() -> int:
    if not SCENE_SOURCE.is_file() or not SCENARIO_SOURCE.is_file():
        raise RuntimeError("the Modelica input API fixture or Rhai verifier is missing")

    os.environ.setdefault("LUNCOSIM_EPHEMERAL_SETTINGS", "1")
    os.environ.setdefault("LUNCOSIM_ISOLATED_RUN", "1")
    test_root = ROOT / "target/scene-tests"
    test_root.mkdir(parents=True, exist_ok=True)
    log_path = test_root / "modelica_input_admission_api.log"

    with TemporaryDirectory(prefix="modelica-input-twin-", dir=test_root) as temporary:
        twin_root = Path(temporary)
        scene_rel = Path("sim/scenes/rocket_engine_observables.usda")
        scene_path = twin_root / scene_rel
        scene_path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(SCENE_SOURCE, scene_path)
        (twin_root / "twin.toml").write_text(
            'name = "ModelicaInputAdmissionGate"\n'
            'version = "0.1.0"\n'
            "\n[usd]\n"
            'default_scene = "sim/scenes/rocket_engine_observables.usda"\n'
            'scenes = ["sim/scenes/*.usda"]\n',
            encoding="utf-8",
        )

        with ProductionSession(PORT, log_path=log_path) as session:
            session_pid = verify_owned_session(session)
            opened = execute(session, "OpenTwin", {"path": str(twin_root)})
            if not opened:
                raise RuntimeError("OpenTwin returned an empty acknowledgement")
            target_gid = wait_for_modelica_fixture(session)

            missing_status, missing_producer = execute_expected_rejection(
                session,
                "SetModelInput",
                {
                    "doc_id": 0,
                    "target_gid": target_gid,
                    "name": INPUT_NAME,
                    "value": INPUT_VALUE,
                },
            )
            if (
                missing_status != 409
                or missing_producer.get("error_code") != 409
                or "producer_id" not in (missing_producer.get("error") or "")
            ):
                raise RuntimeError(
                    f"live API SetModelInput did not reject a missing producer_id: {missing_producer}"
                )

            invalid_status, invalid_target = execute_expected_rejection(
                session,
                "SetModelInput",
                {
                    "doc_id": 0,
                    "target_gid": 0,
                    "name": INPUT_NAME,
                    "value": INPUT_VALUE,
                    "producer_id": PRODUCER_ID,
                },
            )
            if (
                invalid_status != 409
                or invalid_target.get("error_code") != 409
                or "target_gid" not in (invalid_target.get("error") or "")
            ):
                raise RuntimeError(
                    f"live API SetModelInput did not reject a zero target identity: {invalid_target}"
                )

            missing_target_status, missing_target = execute_expected_rejection(
                session,
                "SetModelInput",
                {
                    "doc_id": 0,
                    "target_gid": 18446744073709551615,
                    "name": INPUT_NAME,
                    "value": INPUT_VALUE,
                    "producer_id": PRODUCER_ID,
                },
            )
            if (
                missing_target_status != 409
                or missing_target.get("error_code") != 409
                or "does not resolve" not in (missing_target.get("error") or "")
            ):
                raise RuntimeError(
                    f"live API SetModelInput did not reject an unresolved target: {missing_target}"
                )

            conflicting_target_status, conflicting_target = execute_expected_rejection(
                session,
                "SetModelInput",
                {
                    "doc_id": 1,
                    "target_gid": target_gid,
                    "name": INPUT_NAME,
                    "value": INPUT_VALUE,
                    "producer_id": PRODUCER_ID,
                },
            )
            if (
                conflicting_target_status != 409
                or conflicting_target.get("error_code") != 409
                or "either `doc_id` or `target_gid`"
                not in (conflicting_target.get("error") or "")
            ):
                raise RuntimeError(
                    f"live API SetModelInput did not reject conflicting selectors: {conflicting_target}"
                )

            started = execute(session, "StartSessionInputCapture")
            if started.get("state") not in (None, "recording"):
                raise RuntimeError(f"session input capture did not start: {started}")
            state = execute(session, "ReadSessionInputStream")
            if state.get("state") != "recording":
                raise RuntimeError(f"session input capture is not recording: {state}")

            admission_ack = execute(
                session,
                "SetModelInput",
                {
                    "doc_id": 0,
                    "target_gid": target_gid,
                    "name": INPUT_NAME,
                    "value": INPUT_VALUE,
                    "producer_id": PRODUCER_ID,
                },
            )
            admission = admission_ack.get("admission")
            correlation_id = admission_ack.get("correlation_id")
            if (
                admission_ack.get("target_gid") != target_gid
                or admission_ack.get("producer_kind") != "api_transport"
                or admission_ack.get("producer_id") != PRODUCER_ID
                or not isinstance(admission, dict)
                or not correlation_id
            ):
                raise RuntimeError(f"SetModelInput returned an incomplete live admission: {admission_ack}")

            with SCENARIO_SOURCE.open("r", encoding="utf-8") as source:
                scenario = source.read()
            offset = log_path.stat().st_size
            script = execute(
                session,
                "RunScenario",
                {
                    "target": target_gid,
                    "source": scenario,
                    "params": {
                        "name": INPUT_NAME,
                        "value": INPUT_VALUE,
                        "target_gid": target_gid,
                        "producer_id": PRODUCER_ID,
                        "correlation_id": correlation_id,
                        "admission": admission,
                    },
                },
            )
            if not script:
                raise RuntimeError("RunScenario returned an empty acknowledgement")

            verdict = wait_for_verdict(log_path, offset)
            print("PASS — live Modelica input admission and fixed-tick commit")
            print(f"    api_pid={session_pid} port={PORT} binary={BINARY}")
            print(f"    target={target_gid} admission={admission}")
            print(f"    log={log_path}")
            print(verdict.rstrip())

    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as error:
        print(f"FAIL — {error}", file=sys.stderr)
        raise SystemExit(1) from error
