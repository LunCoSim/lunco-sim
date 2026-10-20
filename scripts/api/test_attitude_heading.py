#!/usr/bin/env python3
"""Run analytic attitude-heading witnesses through the production Modelica owner."""
import argparse
import json
import math
import os
import time
from pathlib import Path

from runtime import ROOT, ProductionSession, run_authored_scenario


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', type=int, default=4292)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    os.environ.update(LUNCOSIM_CONFIG=str(args.output / 'config'),
                      LUNCOSIM_EPHEMERAL_SETTINGS='1')
    source = ROOT / 'assets/models/tests/AttitudeHeadingContracts.mo'
    with ProductionSession(args.port,
            extra_args=['--scene', 'scenes/tests/modelica_runtime_contracts.usda'],
            log_path=args.output / 'runtime.log') as session:
        def execute(command, **params):
            response = session.post(dict(type='ExecuteCommand', command=command, params=params))
            if response.get('error'):
                raise RuntimeError(response)
            return response.get('data', response)

        doc = execute('CreateScratchModelicaDocument', name=source.stem,
                      source=source.read_text())['doc_id']
        deadline = time.monotonic() + 60
        while not execute('CompileStatus', doc_id=doc)['ast_parsed']:
            if time.monotonic() > deadline:
                raise RuntimeError('Heading fixture did not parse')
            time.sleep(.1)
        generation = execute('GetDocumentSource', doc_id=doc)['generation']
        runs = []
        for name, heading, valid, ex, ey in [
            ('Aligned', 0., 1., 0., 0.), ('Right', math.pi/2, 1., 0., math.pi/2),
            ('Left', -math.pi/2, 1., 0., -math.pi/2),
            ('Behind', math.pi, 1., 0., math.pi), ('Disabled', math.pi/2, 1., 0., 0.),
            ('Overhead', 0., 0., 0., 0.), ('Tilted', 0., 1., -.5, 0.),
            ('Yawed', -math.pi/2, 1., 0., -math.pi/2), ('Inverted', -math.pi/2, 1., 0., 0.),
        ]:
            run = execute('RunModelicaSolve', doc_id=doc, **{
                'class': 'AttitudeHeadingContracts.' + name, 'source_generation': generation,
                't_start': 0., 't_end': .2, 'dt': .1, 'label': 'Attitude heading ' + name})
            runs.append(dict(id=run['experiment_id'], name=name, heading=heading,
                             valid=valid, ex=ex, ey=ey))
        (args.output / 'runs.json').write_text(json.dumps(runs, indent=2))
        execute('SetTimeTransport', playing=True)
        run_authored_scenario(session, 'lunco://scenarios/tests/attitude_heading_contracts.rhai',
                              title='ATTITUDE HEADING', check_count=63, params=dict(runs=runs))
        for run in runs:
            result = execute('GetExperimentResult', experiment_id=run['id'],
                             variables=['heading','valid','ex','ey','ez'], max_points=100)
            (args.output / (run['name'] + '.json')).write_text(json.dumps(result, indent=2))
    print('PASS 63 authored attitude heading checks; owned API released')


if __name__ == '__main__':
    main()
