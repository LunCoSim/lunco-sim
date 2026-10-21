#!/usr/bin/env python3
"""Check production Modelica flight-command availability through power interruption."""
import argparse
import json
import os
import shutil
import time
from pathlib import Path

from runtime import ROOT, ProductionSession, port_is_open, run_authored_scenario


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', type=int, default=4293)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    os.environ['LUNCOSIM_CONFIG'] = str(output / 'config')
    fields = ['throttle', 'torque_x', 'torque_y', 'torque_z']
    with ProductionSession(args.port, windowed=False,
                           extra_args=('--scene', 'scenes/base/lunar_surface.usda'),
                           log_path=output / 'runtime.log') as session:
        def execute(command, **params):
            response = session.post({'type': 'ExecuteCommand', 'command': command, 'params': params})
            if response.get('error'):
                raise RuntimeError(response)
            return response.get('data', response)

        source = ROOT / 'assets/models/tests/LanderPowerContracts.mo'
        doc = execute('CreateScratchModelicaDocument', source=(ROOT / 'assets/models/Lander.mo').read_text()+'\n'+source.read_text(),
                      name=source.stem)['doc_id']
        deadline = time.monotonic() + 90
        while not execute('CompileStatus', doc_id=doc)['ast_parsed']:
            assert time.monotonic() < deadline, 'source parse timed out'
            time.sleep(.1)
        generation = execute('GetDocumentSource', doc_id=doc)['generation']
        run = execute('RunModelicaSolve', doc_id=doc, **{
            'class': source.stem, 'source_generation': generation,
            't_start': 0., 't_end': .6, 'dt': .01, 'label': 'Flight command power availability'})
        run_id = run['experiment_id']
        execute('SetTimeTransport', playing=True)
        run_authored_scenario(session, 'lunco://scenarios/tests/lander_power_contracts.rhai',
                              title='LANDER POWER CONTRACTS', check_count=108,
                              params={'contract_run': run_id})
        names = [f'{group}.{field}' for group in ['powered', 'off', 'recovering'] for field in fields]
        result = execute('GetExperimentResult', experiment_id=run_id, variables=names, max_points=64)
        (output / 'trajectory.json').write_text(json.dumps(result, indent=2))
        execute('CloseDocument', doc_id=doc)
    artifact_root = ROOT / 'assets/scenes/base/results'
    artifact = artifact_root / f'{run_id}.json'
    if artifact.is_file():
        shutil.move(str(artifact), output / artifact.name)
    if artifact_root.is_dir() and not any(artifact_root.iterdir()):
        artifact_root.rmdir()
    evidence = {'checks': 108, 'samples': len(result['times']), 'verdict': 'pass',
                'port_released': not port_is_open(args.port)}
    (output / 'validation.json').write_text(json.dumps(evidence, indent=2))
    assert evidence['port_released']
    print('PASS 108 authored flight-command power checks; owned API released')


if __name__ == '__main__':
    main()
