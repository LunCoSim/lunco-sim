#!/usr/bin/env python3
"""Exercise finite data storage and powered equipment through production Modelica."""
import argparse
import json
import os
import shutil
import time
from pathlib import Path
from runtime import ROOT, ProductionSession, port_is_open, run_authored_scenario

TARGETS = ['StoragePowerContracts', 'StoreZeroCapacity', 'StoreInvalidInitialFill',
           'StoreNegativeRequest', 'StoreDynamicNegativeRequest', 'MemoryNegativePower',
           'RadioInvalidEfficiency', 'LoadInvalidHysteresis', 'CameraInvalidPower',
           'RadioNegativePower', 'LinkInvalidCapacity', 'LinkInvalidTime']
CHECKS = 67


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', type=int, default=4193)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--headless', action='store_true')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    os.environ['LUNCOSIM_CONFIG'] = str(output / 'config')
    runs = []
    try:
        with ProductionSession(args.port, windowed=not args.headless,
                               extra_args=('--scene', 'scenes/base/lunar_surface.usda'),
                               log_path=output / 'runtime.log') as session:
            def execute(command, **params):
                response = session.post({'type': 'ExecuteCommand', 'command': command, 'params': params})
                if response.get('error'):
                    raise RuntimeError(response)
                return response.get('data', response)
            source = ROOT / 'assets/models/tests/StoragePowerContracts.mo'
            doc = execute('CreateScratchModelicaDocument', source=source.read_text(),
                          name=source.stem)['doc_id']
            deadline = time.monotonic() + 60
            while not execute('CompileStatus', doc_id=doc)['ast_parsed']:
                if time.monotonic() > deadline:
                    raise RuntimeError('Storage/power contract source did not parse')
                time.sleep(.1)
            generation = execute('GetDocumentSource', doc_id=doc)['generation']
            for target in TARGETS:
                run = execute('RunModelicaSolve', doc_id=doc,
                              **{'class': target, 'source_generation': generation,
                                 't_start': 0., 't_end': 7., 'dt': .01, 'label': target})
                runs.append(run['experiment_id'])
            execute('SetTimeTransport', playing=True)
            try:
                run_authored_scenario(session, 'lunco://scenarios/tests/storage_power_contracts.rhai',
                                      title='STORAGE POWER', check_count=CHECKS,
                                      params={'run': runs[0], 'invalid_runs': runs[1:]})
            finally:
                execute('CloseDocument', doc_id=doc)
    finally:
        artifact_root = ROOT / 'assets/scenes/base/results'
        for run in runs:
            artifact = artifact_root / f'{run}.json'
            if artifact.is_file():
                shutil.move(str(artifact), output / artifact.name)
        if artifact_root.is_dir() and not any(artifact_root.iterdir()):
            artifact_root.rmdir()
    evidence = {'checks': CHECKS, 'headless': args.headless, 'runs': runs,
                'port_released': not port_is_open(args.port)}
    (output / 'validation.json').write_text(json.dumps(evidence, indent=2))
    assert evidence['port_released']
    print(f'PASS {CHECKS} authored storage/power checks; owned API Exit and port release')


if __name__ == '__main__':
    main()
