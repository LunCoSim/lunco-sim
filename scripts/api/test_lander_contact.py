#!/usr/bin/env python3
"""Run contact-phase witnesses against the production Modelica controller."""
import argparse
import json
import os
import shutil
import time
from pathlib import Path

from runtime import ROOT, ProductionSession, run_authored_scenario


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', type=int, default=4294)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    os.environ.update(LUNCOSIM_CONFIG=str(args.output / 'config'),
                      LUNCOSIM_EPHEMERAL_SETTINGS='1')
    with ProductionSession(args.port, windowed=False,
            extra_args=['--scene', 'scenes/tests/modelica_runtime_contracts.usda'],
            log_path=args.output / 'runtime.log') as session:
        def execute(command, **params):
            response = session.post(dict(type='ExecuteCommand', command=command, params=params))
            if response.get('error'):
                raise RuntimeError(response)
            return response.get('data', response)

        fixture = ROOT / 'assets/models/tests/LanderContactContracts.mo'
        source = (ROOT / 'assets/models/Lander.mo').read_text() + '\n' + fixture.read_text()
        doc = execute('CreateScratchModelicaDocument', name=fixture.stem, source=source)['doc_id']
        deadline = time.monotonic() + 60
        while not execute('CompileStatus', doc_id=doc)['ast_parsed']:
            if time.monotonic() > deadline:
                raise RuntimeError('Contact fixture did not parse')
            time.sleep(.1)
        generation = execute('GetDocumentSource', doc_id=doc)['generation']
        runs = []
        for name, cutoff, settled, throttle in [
            ('SoftContact', 1., 0., .6), ('Airborne', 0., 0., .6),
            ('FastSpin', 0., 0., .6), ('FastGround', 0., 0., .6),
            ('CutoffRateBoundary', 1., 0., .6), ('StricterCutoffRate', 0., 0., .6),
            ('FastDescent', 0., 0., .6), ('Tipped', 0., 0., .6),
            ('QuietFourPads', 1., 1., .6), ('MovingStrut', 1., 0., .6),
            ('SettledRateBoundary', 1., 1., .6),
            ('AcceptedCutoff', 1., 0., 0.), ('AcceptedHandoff', 1., 0., 0.),
        ]:
            run = execute('RunModelicaSolve', doc_id=doc, **{
                'class': fixture.stem + '.' + name, 'source_generation': generation,
                't_start': 0., 't_end': .2, 'dt': .1, 'label': name})
            runs.append(dict(id=run['experiment_id'], name=name,
                             cutoff=cutoff, settled=settled, throttle=throttle))
        (args.output / 'runs.json').write_text(json.dumps(runs, indent=2))
        execute('SetTimeTransport', playing=True)
        run_authored_scenario(session, 'lunco://scenarios/tests/lander_contact_contracts.rhai',
                              title='LANDER CONTACT CONTRACTS', check_count=78, params=dict(runs=runs))
        for run in runs:
            result = execute('GetExperimentResult', experiment_id=run['id'],
                             variables=['cutoff','settled','throttle','torque'], max_points=100)
            (args.output / (run['name'] + '.json')).write_text(json.dumps(result, indent=2))
        execute('CloseDocument', doc_id=doc)
    # Experiment persistence belongs to the selected evidence directory.
    results = ROOT / 'assets/scenes/tests/results'
    for run in runs:
        artifact = results / (run['id'] + '.json')
        if artifact.is_file():
            shutil.move(str(artifact), args.output / artifact.name)
    if results.is_dir() and not any(results.iterdir()):
        results.rmdir()
    print('PASS 78 authored contact-phase checks; owned API released')


if __name__ == '__main__':
    main()
