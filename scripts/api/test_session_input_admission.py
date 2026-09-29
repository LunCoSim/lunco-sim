#!/usr/bin/env python3
"""Verify external API port writes and releases through fixed-tick admission."""

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
CONTROL_SCENE_SOURCE = ROOT / "assets/scenes/terrain_only.usda"
SCENARIO_SOURCE = ROOT / "assets/scenarios/tests/session_input_admission.rhai"
RELEASE_SCENARIO_SOURCE = ROOT / "assets/scenarios/tests/session_control_release_admission.rhai"
AUTHORITY_RELEASE_SCENARIO_SOURCE = ROOT / "assets/scenarios/tests/session_authority_release_preserves_ports.rhai"
TIMEOUT_S = float(os.environ.get("SESSION_INPUT_TIMEOUT", "240"))
PORT = int(os.environ.get("SESSION_INPUT_API_PORT", "4732"))
PRODUCER_ID = 8182
PORT_PRODUCER_ID = 8282
RELEASE_WRITE_PRODUCER_ID = 8382
RELEASE_PORT_PRODUCER_ID = 8383
RELEASE_CONTROL_PRODUCER_ID = 8484
AUTHORITY_PORT_PRODUCER_ID = 8585
INPUT_NAME = "throttle"
INPUT_VALUE = 0.375
PORT_VALUE = 0.625
RELEASE_WRITE_VALUE = 0.875
AUTHORITY_PORT_WRITES = {
    "forward": 0.7,
    "side": -0.25,
    "up": 0.4,
    "speed_boost": 1.0,
}


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


def wait_for_modelica_fixture(session: ProductionSession) -> tuple[int, int, dict]:
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
                target_gid = int(target["api_id"])
                status, rejection = execute_expected_rejection(
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
                    status == 409
                    and rejection.get("error_code") == 409
                    and "producer_id" in (rejection.get("error") or "")
                ):
                    return target_gid, status, rejection
                if status == 200:
                    raise RuntimeError(
                        "live API SetModelInput accepted a missing producer_id"
                    )
                if "ModelicaModel" not in (rejection.get("error") or ""):
                    raise RuntimeError(
                        f"SetModelInput target readiness check failed: {rejection}"
                    )
        time.sleep(0.25)
    raise RuntimeError(
        f"RocketEngineObservables did not expose a compiled Modelica input owner within {TIMEOUT_S:g}s"
    )


def wait_for_input_value(
    session: ProductionSession, target_gid: int, name: str, value: float
) -> None:
    deadline = time.monotonic() + TIMEOUT_S
    while time.monotonic() < deadline:
        ports = execute(session, "ReadPorts", {"api_id": target_gid}).get("ports") or []
        if any(
            port.get("name") == name
            and port.get("direction") == "in"
            and port.get("value") == value
            for port in ports
        ):
            return
        time.sleep(0.02)
    raise RuntimeError(
        f"live Modelica input {name!r} did not reach {value!r} at its admitted fixed tick"
    )


def wait_for_control_endpoint(session: ProductionSession) -> int:
    deadline = time.monotonic() + TIMEOUT_S
    last_entities: list[dict] = []
    required_ports = set(AUTHORITY_PORT_WRITES)
    while time.monotonic() < deadline:
        entities = response_data(session.post({"type": "ListEntities"}), "ListEntities")[
            "entities"
        ]
        last_entities = entities
        for entity in entities:
            target_gid = int(entity["api_id"])
            ports_response = session.post(
                {
                    "type": "ExecuteCommand",
                    "command": "ReadPorts",
                    "params": {"api_id": target_gid},
                }
            )
            if ports_response.get("error"):
                continue
            ports = (ports_response.get("data") or {}).get("ports") or []
            writable_names = {
                port.get("name")
                for port in ports
                if port.get("direction") == "in"
            }
            if required_ports.issubset(writable_names):
                return target_gid
        time.sleep(0.25)
    candidates = [
        f"{entity.get('name')} (control_bound={entity.get('control_bound')})"
        for entity in last_entities
    ]
    raise RuntimeError(
        "control scene did not expose the complete avatar command endpoint within "
        f"{TIMEOUT_S:g}s; entities={candidates}"
    )


def wait_for_port_values(
    session: ProductionSession, target_gid: int, expected: dict[str, float]
) -> None:
    deadline = time.monotonic() + TIMEOUT_S
    while time.monotonic() < deadline:
        ports = execute(session, "ReadPorts", {"api_id": target_gid}).get("ports") or []
        actual = {
            port.get("name"): port.get("value")
            for port in ports
            if port.get("direction") == "in"
        }
        if all(actual.get(name) == value for name, value in expected.items()):
            return
        time.sleep(0.02)
    raise RuntimeError(
        f"live command values did not reach their admitted state: {expected}"
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
        if "SESSION_INPUT_ADMISSION" in output:
            if "TESTS_FAIL" in output:
                raise RuntimeError(f"session input Rhai gate failed:\n{output}")
            if "TESTS_OK" in output:
                return output
        time.sleep(0.25)
    raise RuntimeError(f"session input Rhai gate produced no verdict; log={log_path}")


def main() -> int:
    fixtures = (
        SCENE_SOURCE,
        CONTROL_SCENE_SOURCE,
        SCENARIO_SOURCE,
        RELEASE_SCENARIO_SOURCE,
        AUTHORITY_RELEASE_SCENARIO_SOURCE,
    )
    if not all(source.is_file() for source in fixtures):
        raise RuntimeError("an input-admission fixture or Rhai verifier is missing")

    os.environ.setdefault("LUNCOSIM_EPHEMERAL_SETTINGS", "1")
    os.environ.setdefault("LUNCOSIM_ISOLATED_RUN", "1")
    test_root = ROOT / "target/scene-tests"
    test_root.mkdir(parents=True, exist_ok=True)
    log_path = test_root / "session_input_admission_api.log"

    with TemporaryDirectory(prefix="session-input-twin-", dir=test_root) as temporary:
        twin_root = Path(temporary)
        scene_rel = Path("sim/scenes/rocket_engine_observables.usda")
        scene_path = twin_root / scene_rel
        scene_path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(SCENE_SOURCE, scene_path)
        # Give the control-profile fixture a task-owned Twin-relative path.
        control_scene_rel = Path("sim/scenes/lifecycle_controls.usda")
        shutil.copy2(CONTROL_SCENE_SOURCE, twin_root / control_scene_rel)
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
            target_gid, missing_status, missing_producer = wait_for_modelica_fixture(session)
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

            missing_port_producer_status, missing_port_producer = execute_expected_rejection(
                session,
                "SetPorts",
                {"target": target_gid, "writes": [[INPUT_NAME, PORT_VALUE]]},
            )
            if (
                missing_port_producer_status != 409
                or missing_port_producer.get("error_code") != 409
                or "producer_id" not in (missing_port_producer.get("error") or "")
            ):
                raise RuntimeError(
                    f"live API SetPorts did not reject a missing producer_id: {missing_port_producer}"
                )

            unknown_port_status, unknown_port = execute_expected_rejection(
                session,
                "SetPorts",
                {
                    "target": target_gid,
                    "writes": [["unknown_input", PORT_VALUE]],
                    "producer_id": PORT_PRODUCER_ID,
                },
            )
            if (
                unknown_port_status != 409
                or unknown_port.get("error_code") != 409
                or "unknown input port" not in (unknown_port.get("error") or "")
            ):
                raise RuntimeError(
                    f"live API SetPorts did not reject an unknown input port: {unknown_port}"
                )

            missing_release_port_status, missing_release_port = execute_expected_rejection(
                session,
                "ReleasePort",
                {"target": target_gid, "name": INPUT_NAME},
            )
            if (
                missing_release_port_status != 409
                or missing_release_port.get("error_code") != 409
                or "producer_id" not in (missing_release_port.get("error") or "")
            ):
                raise RuntimeError(
                    f"live API ReleasePort did not reject a missing producer_id: {missing_release_port}"
                )

            missing_release_control_status, missing_release_control = execute_expected_rejection(
                session,
                "ReleaseControl",
                {"target": target_gid},
            )
            if (
                missing_release_control_status != 409
                or missing_release_control.get("error_code") != 409
                or "producer_id" not in (missing_release_control.get("error") or "")
            ):
                raise RuntimeError(
                    "live API ReleaseControl did not reject a missing producer_id: "
                    f"{missing_release_control}"
                )

            started = execute(session, "StartSessionInputCapture")
            if started.get("state") not in (None, "recording"):
                raise RuntimeError(f"session input capture did not start: {started}")
            state = execute(session, "ReadSessionInputStream")
            if state.get("state") != "recording":
                raise RuntimeError(f"session input capture is not recording: {state}")
            execute(session, "SetTimeTransport", {"playing": False})

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

            execute(session, "SetTimeTransport", {"playing": True})
            wait_for_input_value(session, target_gid, INPUT_NAME, INPUT_VALUE)
            execute(session, "SetTimeTransport", {"playing": False})

            port_admission_ack = execute(
                session,
                "SetPorts",
                {
                    "target": target_gid,
                    "writes": [[INPUT_NAME, PORT_VALUE]],
                    "producer_id": PORT_PRODUCER_ID,
                },
            )
            port_admission = port_admission_ack.get("admission")
            port_correlation_id = port_admission_ack.get("correlation_id")
            if (
                port_admission_ack.get("target_gid") != target_gid
                or port_admission_ack.get("producer_kind") != "api_transport"
                or port_admission_ack.get("producer_id") != PORT_PRODUCER_ID
                or not isinstance(port_admission, dict)
                or not port_correlation_id
            ):
                raise RuntimeError(f"SetPorts returned an incomplete live admission: {port_admission_ack}")

            pending_stop_status, pending_stop = execute_expected_rejection(
                session, "StopSessionInputCapture", {}
            )
            if (
                pending_stop_status != 409
                or pending_stop.get("error_code") != 409
                or "admitted inputs are pending" not in (pending_stop.get("error") or "")
            ):
                raise RuntimeError(
                    "StopSessionInputCapture did not reject pending admitted inputs: "
                    f"{pending_stop}"
                )
            execute(session, "SetTimeTransport", {"playing": True})

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
                        "port_producer_id": PORT_PRODUCER_ID,
                        "port_correlation_id": port_correlation_id,
                        "port_admission": port_admission,
                        "port_value": PORT_VALUE,
                    },
                },
            )
            if not script:
                raise RuntimeError("RunScenario returned an empty acknowledgement")

            verdict = wait_for_verdict(log_path, offset)
            execute(session, "SetTimeTransport", {"playing": False})
            execute(session, "ClearSessionInputCapture")
            execute(session, "StartSessionInputCapture")
            release_write_ack = execute(
                session,
                "SetPorts",
                {
                    "target": target_gid,
                    "writes": [[INPUT_NAME, RELEASE_WRITE_VALUE]],
                    "producer_id": RELEASE_WRITE_PRODUCER_ID,
                },
            )
            port_release_ack = execute(
                session,
                "ReleasePort",
                {
                    "target": target_gid,
                    "name": INPUT_NAME,
                    "producer_id": RELEASE_PORT_PRODUCER_ID,
                },
            )
            control_release_ack = execute(
                session,
                "ReleaseControl",
                {"target": target_gid, "producer_id": RELEASE_CONTROL_PRODUCER_ID},
            )
            for command, ack, producer_id in (
                ("SetPorts before release", release_write_ack, RELEASE_WRITE_PRODUCER_ID),
                ("ReleasePort", port_release_ack, RELEASE_PORT_PRODUCER_ID),
                ("ReleaseControl", control_release_ack, RELEASE_CONTROL_PRODUCER_ID),
            ):
                if (
                    ack.get("target_gid") != target_gid
                    or ack.get("producer_kind") != "api_transport"
                    or ack.get("producer_id") != producer_id
                    or not isinstance(ack.get("admission"), dict)
                    or not ack.get("correlation_id")
                ):
                    raise RuntimeError(f"{command} returned an incomplete live admission: {ack}")
            release_stamps = [
                ack["admission"]
                for ack in (release_write_ack, port_release_ack, control_release_ack)
            ]
            if (
                len({stamp.get("effective_tick") for stamp in release_stamps}) != 1
                or not (
                    release_stamps[0].get("sequence")
                    < release_stamps[1].get("sequence")
                    < release_stamps[2].get("sequence")
                )
            ):
                raise RuntimeError(
                    "same-tick release inputs did not retain their shared sequence order: "
                    f"{release_stamps}"
                )

            with RELEASE_SCENARIO_SOURCE.open("r", encoding="utf-8") as source:
                release_scenario = source.read()
            release_offset = log_path.stat().st_size
            release_script = execute(
                session,
                "RunScenario",
                {
                    "target": target_gid,
                    "source": release_scenario,
                    "params": {
                        "target_gid": target_gid,
                        "write_producer_id": RELEASE_WRITE_PRODUCER_ID,
                        "write_correlation_id": release_write_ack["correlation_id"],
                        "write_admission": release_write_ack["admission"],
                        "write_value": RELEASE_WRITE_VALUE,
                        "port_producer_id": RELEASE_PORT_PRODUCER_ID,
                        "port_correlation_id": port_release_ack["correlation_id"],
                        "port_admission": port_release_ack["admission"],
                        "control_producer_id": RELEASE_CONTROL_PRODUCER_ID,
                        "control_correlation_id": control_release_ack["correlation_id"],
                        "control_admission": control_release_ack["admission"],
                    },
                },
            )
            if not release_script:
                raise RuntimeError("RunScenario returned an empty release-verifier acknowledgement")
            execute(session, "SetTimeTransport", {"playing": True})
            release_verdict = wait_for_verdict(log_path, release_offset)

            execute(session, "ClearSessionInputCapture")
            execute(
                session,
                "LoadScene",
                {
                    "path": "twin://ModelicaInputAdmissionGate/"
                    "sim/scenes/lifecycle_controls.usda"
                },
            )
            authority_target_gid = wait_for_control_endpoint(session)
            execute(session, "SetTimeTransport", {"playing": True})
            execute(session, "StartSessionInputCapture")
            authority_port_ack = execute(
                session,
                "SetPorts",
                {
                    "target": authority_target_gid,
                    "writes": [
                        [name, value]
                        for name, value in AUTHORITY_PORT_WRITES.items()
                    ],
                    "producer_id": AUTHORITY_PORT_PRODUCER_ID,
                },
            )
            authority_port_admission = authority_port_ack.get("admission")
            authority_port_correlation_id = authority_port_ack.get("correlation_id")
            if (
                authority_port_ack.get("target_gid") != authority_target_gid
                or authority_port_ack.get("producer_kind") != "api_transport"
                or authority_port_ack.get("producer_id") != AUTHORITY_PORT_PRODUCER_ID
                or not isinstance(authority_port_admission, dict)
                or not authority_port_correlation_id
            ):
                raise RuntimeError(
                    "authority-release SetPorts returned an incomplete live admission: "
                    f"{authority_port_ack}"
                )
            wait_for_port_values(session, authority_target_gid, AUTHORITY_PORT_WRITES)
            claim_ack = execute(
                session, "ClaimControl", {"target": authority_target_gid}
            )
            authority_release_ack = execute(
                session, "ReleaseControlClaim", {"target": authority_target_gid}
            )
            if not claim_ack or not authority_release_ack:
                raise RuntimeError("control-authority transition returned an empty acknowledgement")
            with AUTHORITY_RELEASE_SCENARIO_SOURCE.open("r", encoding="utf-8") as source:
                authority_scenario = source.read()
            authority_offset = log_path.stat().st_size
            authority_script = execute(
                session,
                "RunScenario",
                {
                    "target": authority_target_gid,
                    "source": authority_scenario,
                    "params": {
                        "target_gid": authority_target_gid,
                        "producer_id": AUTHORITY_PORT_PRODUCER_ID,
                        "correlation_id": authority_port_correlation_id,
                        "admission": authority_port_admission,
                    },
                },
            )
            if not authority_script:
                raise RuntimeError(
                    "RunScenario returned an empty authority-release verifier acknowledgement"
                )
            authority_verdict = wait_for_verdict(log_path, authority_offset)

            print(
                "PASS — Modelica, SetPorts, explicit releases, and authority-release port preservation"
            )
            print(f"    api_pid={session_pid} port={PORT} binary={BINARY}")
            print(f"    target={target_gid} Modelica={admission} SetPorts={port_admission}")
            print(
                f"    authority release target={authority_target_gid} "
                f"SetPorts={authority_port_admission}"
            )
            print("    Modelica input was observed at its admitted tick before the later SetPorts write")
            print(
                "    releases="
                f"{port_release_ack['admission']} / {control_release_ack['admission']}"
            )
            print(f"    log={log_path}")
            print(verdict.rstrip())
            print(release_verdict.rstrip())
            print(authority_verdict.rstrip())

    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as error:
        print(f"FAIL — {error}", file=sys.stderr)
        raise SystemExit(1) from error
