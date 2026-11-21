#!/usr/bin/env python3
"""Exercise Twin namespace publication through a generated Modelica network."""

from __future__ import annotations

import argparse
import os
import subprocess
from pathlib import Path

from runtime import BINARY, ROOT


def create_twin(twin: Path, *, expose_dependency: bool) -> Path:
    scenes = twin / "scenes"
    package = twin / "models/StartupPower"
    slow_sources = twin / "models/slow"
    for directory in (scenes, package, slow_sources):
        directory.mkdir(parents=True)

    paths = '["models"]' if expose_dependency else '["models/slow"]'
    scene_name = "powered_equipment_assembly.usda"
    (twin / "twin.toml").write_text(
        f'name = "Modelica namespace startup fixture"\nversion = "0.1.0"\n\n'
        f'[usd]\ndefault_scene = "scenes/{scene_name}"\n\n'
        f'[modelica]\npaths = {paths}\n',
        encoding="utf-8",
    )

    # Keep source-set parsing in flight as the scene projects its participants.
    # The dependency's name comes from Modelica, independently of the folder.
    for index in range(320):
        name = f"StartupLoad{index:04d}"
        parameters = "".join(f"  parameter Real p{k} = {index} + {k};\n" for k in range(36))
        (slow_sources / f"{name}.mo").write_text(
            f"within;\nmodel {name}\n{parameters}end {name};\n", encoding="utf-8"
        )
    (package / "package.mo").write_text(
        "within;\npackage StartupPower\nend StartupPower;\n", encoding="utf-8"
    )
    rail = (ROOT / "assets/models/LunCo/Electrical/IdealVoltageSource.mo").read_text(encoding="utf-8")
    assert rail.startswith("within LunCo.Electrical;") and "  Pin p;" in rail
    (package / "IdealVoltageSource.mo").write_text(
        rail.replace("within LunCo.Electrical;", "within StartupPower;", 1)
        .replace("  Pin p;", "  LunCo.Electrical.Pin p;", 1),
        encoding="utf-8",
    )
    scene = scenes / scene_name
    source = (ROOT / "assets/scenes/tests" / scene_name).read_text(encoding="utf-8")
    asset = "@lunco://models/LunCo/Electrical/IdealVoltageSource.mo@"
    assert source.count(asset) == 1
    scene.write_text(
        source.replace(asset, "@../models/StartupPower/IdealVoltageSource.mo@"),
        encoding="utf-8",
    )
    return scene


def run_case(folder: Path, *, expose_dependency: bool) -> None:
    scene = create_twin(folder / "twin", expose_dependency=expose_dependency)
    environment = os.environ | {
        "LUNCO_ASSET_ROOT": str(ROOT / "assets"),
        "LUNCOSIM_CONFIG": str(folder / "config"),
        "XDG_CACHE_HOME": str(folder / "cache"),
        "LUNCOSIM_EPHEMERAL_SETTINGS": "1",
        "LUNCOSIM_ISOLATED_RUN": "1",
    }
    log = folder / "native.log"
    with log.open("wb") as stream:
        result = subprocess.run(
            [str(BINARY), "test", "--scene", str(scene), "--threads", "1", "--jitter", "0",
             "--max-ticks", "12000", "--verdict-channel", "POWERED_EQUIPMENT"],
            cwd=ROOT, env=environment, stdin=subprocess.DEVNULL,
            stdout=stream, stderr=subprocess.STDOUT, timeout=120,
        )
    output = log.read_text(encoding="utf-8")
    ready = output.find('is now Ready namespaces=["StartupLoad0000"')
    diagnostic = "Modelica source root `StartupPower` is not registered"
    if expose_dependency:
        passed = (result.returncode == 0 and "TESTS_OK 12" in output
                  and "luncosim test PASS" in output and diagnostic not in output)
    else:
        passed = (result.returncode == 1 and 0 <= ready < output.find(diagnostic)
                  and "terminal runtime fault kind=modelica-compile-failed" in output)
    if not passed:
        raise RuntimeError(f"Unexpected namespace startup result (exit {result.returncode}). See {log}")
    print(f"PASS {folder.name} ({log})")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifacts", type=Path, required=True, help="New evidence directory")
    args = parser.parse_args()
    artifacts = args.artifacts.resolve()
    artifacts.mkdir(parents=True)
    run_case(artifacts / "declared", expose_dependency=True)
    run_case(artifacts / "undeclared", expose_dependency=False)


if __name__ == "__main__":
    main()
