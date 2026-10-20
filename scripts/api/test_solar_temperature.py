#!/usr/bin/env python3
"""Qualify temperature-sensitive photovoltaics and bounded cooling in production."""
import argparse
import hashlib
import json
import os
import time
from pathlib import Path

from runtime import BINARY, ROOT, ProductionSession, port_is_open, run_authored_scenario

STATIC = ['available_w', 'delivered_w', 'heat_w', 'factor', 'energy_balance_w']
DYNAMIC = ['temperature_k', 'available_w', 'heat_w', 'cooling_w',
           'generated_heat_j', 'rejected_heat_j', 'thermal_balance_j']
STATIC_CASES = ['Reference', 'Hot', 'Cold', 'HotNameplate', 'ColdNameplate',
                'HighBus', 'ExtremeHot']
DYNAMIC_CASES = ['StrongCooling', 'WeakCooling', 'Cooldown', 'BelowSink', 'DisconnectedHeat']
INVALID = {'InvalidTemperature': 'positive absolute cell temperature',
           'InvalidCooling': 'LimitedCoolingSink requires'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', type=int, default=4197)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--headless', action='store_true')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    os.environ['LUNCOSIM_CONFIG'] = str(output / 'config')
    os.environ['LUNCOSIM_EPHEMERAL_SETTINGS'] = '1'
    source = ROOT / 'assets/models/tests/SolarTemperatureContracts.mo'
    runs = []
    with ProductionSession(args.port, windowed=not args.headless,
                           extra_args=('--scene', 'scenes/tests/modelica_runtime_contracts.usda'),
                           log_path=output / 'runtime.log') as session:
        executing_image = Path('/proc') / str(session.process.pid) / 'exe'
        image = executing_image if executing_image.exists() else BINARY
        binary_sha256 = hashlib.file_digest(image.open('rb'), 'sha256').hexdigest()
        def execute(command, **params):
            response = session.post({'type': 'ExecuteCommand', 'command': command, 'params': params})
            if response.get('error'):
                raise RuntimeError(response)
            return response.get('data', response)

        doc = execute('CreateScratchModelicaDocument', name=source.stem,
                      source=source.read_text())['doc_id']
        try:
            deadline = time.monotonic() + 60
            while not execute('CompileStatus', doc_id=doc)['ast_parsed']:
                if time.monotonic() > deadline:
                    raise RuntimeError('Solar temperature fixture did not parse')
                time.sleep(.1)
            generation = execute('GetDocumentSource', doc_id=doc)['generation']
            for target in STATIC_CASES + DYNAMIC_CASES + list(INVALID):
                stop = 300.0 if target in DYNAMIC_CASES else 1.0
                run = execute('RunModelicaSolve', doc_id=doc, **{
                    'class': 'SolarTemperatureContracts.' + target,
                    'source_generation': generation, 't_start': 0.0, 't_end': stop,
                    'dt': 1.0 if target in DYNAMIC_CASES else .1,
                    'label': 'Solar temperature ' + target,
                })
                runs.append(dict(run, label=target, stop_s=stop,
                                 variables=DYNAMIC if target in DYNAMIC_CASES else STATIC,
                                 invalid=target in INVALID, diagnostic=INVALID.get(target, '')))
            (output / 'runs.json').write_text(json.dumps(runs, indent=2))
            execute('SetTimeTransport', playing=True)
            run_authored_scenario(session, 'lunco://scenarios/tests/solar_temperature_contracts.rhai',
                                  title='SOLAR TEMPERATURE', check_count=45, params={'runs': runs})
        finally:
            for run in runs:
                status = execute('RunStatus', experiment_id=run['experiment_id'])
                (output / (run['label'] + '-status.json')).write_text(json.dumps(status, indent=2))
                if status['state'] == 'done':
                    result = execute('GetExperimentResult', experiment_id=run['experiment_id'],
                                     variables=run['variables'], max_points=4000)
                    (output / (run['label'] + '-result.json')).write_text(json.dumps(result, indent=2))
                elif status['state'] not in ('failed', 'cancelled'):
                    execute('CancelExperiment', experiment_id=run['experiment_id'], all=False)
            execute('CloseDocument', doc_id=doc)
    files = [source, ROOT / 'assets/models/LunCo/Electrical/SolarPanel.mo',
             ROOT / 'assets/models/LunCo/Thermal/LimitedCoolingSink.mo']
    evidence = {'checks': 45, 'runs': runs, 'headless': args.headless,
                'binary_sha256': binary_sha256,
                'source_sha256': {str(p.relative_to(ROOT)): hashlib.file_digest(p.open('rb'), 'sha256').hexdigest()
                                  for p in files}, 'port_released': not port_is_open(args.port)}
    (output / 'validation.json').write_text(json.dumps(evidence, indent=2))
    assert evidence['port_released']
    print('PASS 45 authored solar temperature checks; owned API Exit and port release')


if __name__ == '__main__':
    main()
