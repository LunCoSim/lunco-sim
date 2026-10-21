#!/usr/bin/env python3
"""Run the shared radiative exchange contracts through the production host."""
import argparse
import hashlib
import json
import os
import time
from pathlib import Path

from runtime import ROOT, ProductionSession, port_is_open, run_authored_scenario


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    os.environ.update(LUNCOSIM_EPHEMERAL_SETTINGS="1", LUNCOSIM_CONFIG=str(output / "config"))
    fixture = ROOT / "assets/models/tests/RadiativeExchangeContracts.mo"
    with ProductionSession(args.port, windowed=True,
            extra_args=("--scene", str(ROOT / "assets/scenes/tests/modelica_runtime_contracts.usda")),
            log_path=output / "runtime.log") as session:
        def execute(command, **params):
            return session.post({"type": "ExecuteCommand", "command": command, "params": params})["data"]
        doc = execute("CreateScratchModelicaDocument", name="Radiative exchange contracts",
                      source=fixture.read_text())["doc_id"]
        deadline = time.monotonic() + 60
        while not execute("CompileStatus", doc_id=doc)["ast_parsed"]:
            if time.monotonic() > deadline:
                raise RuntimeError("radiative contract source did not parse")
            time.sleep(0.1)
        runs = []
        try:
            for label in ["Cooling", "Heating", "Equilibrium", "Mirror", "InvalidEmissivity"]:
                run = execute("RunExperiment", doc_id=doc, **{
                    "class": "RadiativeExchangeContracts." + label,
                    "t_end": 120.0, "dt": 5.0, "tolerance": 1e-9, "solver": "rk45",
                    "label": "Radiative exchange " + label,
                })
                runs.append(dict(run, label=label))
            (output / "runs.json").write_text(json.dumps(runs, indent=2))
            execute("SetTimeTransport", playing=True)
            run_authored_scenario(session, "lunco://scenarios/tests/radiative_exchange_contracts.rhai",
                title="RADIATIVE EXCHANGE", check_count=17, params={"runs": runs})
        finally:
            for run in runs:
                status = execute("RunStatus", experiment_id=run["experiment_id"])
                (output / (run["label"] + "-status.json")).write_text(json.dumps(status, indent=2))
                if status["state"] == "done":
                    result = execute("GetExperimentResult", experiment_id=run["experiment_id"],
                        variables=["temperature_k", "rejection_w", "balance_w", "exchange_law_w"], max_points=4000)
                    (output / (run["label"] + "-trajectory.json")).write_text(json.dumps(result, indent=2))
            execute("CloseDocument", doc_id=doc)
    assert not port_is_open(args.port)
    (output / "qualification.json").write_text(json.dumps(dict(checks=17, port_released=True,
        radiator_sha256=hashlib.sha256((ROOT / "assets/models/LunCo/Thermal/Radiator.mo").read_bytes()).hexdigest(),
        fixture_sha256=hashlib.sha256(fixture.read_bytes()).hexdigest()), indent=2))
    print("PASS: 17 radiative exchange checks; owned session closed")


if __name__ == "__main__":
    main()
