#!/usr/bin/env python3
"""Run authored Modelica energy/control/connector regressions in production."""
import argparse
import json
import shutil
import os
import time
from pathlib import Path

from runtime import ROOT, ProductionSession, port_is_open, run_authored_scenario


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', type=int, default=4193)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--headless', action='store_true')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    os.environ['LUNCOSIM_CONFIG'] = str(output / 'config')
    with ProductionSession(args.port, windowed=not args.headless,
                           extra_args=('--scene', 'scenes/base/lunar_surface.usda'),
                           log_path=output / 'runtime.log') as session:
        def execute(command, **params):
            response = session.post({'type': 'ExecuteCommand', 'command': command, 'params': params})
            if response.get('error'):
                raise RuntimeError(response)
            return response.get('data', response)

        documents = []
        for source in ['tests/ModelPhysicalContracts.mo', 'WrenchAllocatorContract.mo']:
            path = str(ROOT / 'assets/models' / source)
            scratch = None
            if args.headless:
                scratch = execute('CreateScratchModelicaDocument', source=Path(path).read_text(),
                                  name=Path(path).stem)['doc_id']
            else:
                execute('Open', uri=path)
            deadline = time.monotonic() + 60
            while time.monotonic() < deadline:
                document = next((d for d in execute('ListOpenDocuments')['open_documents']
                                 if d['doc_id'] == scratch or d.get('origin', {}).get('path') == path), None)
                if document and execute('CompileStatus', doc_id=document['doc_id'])['ast_parsed']:
                    documents.append(document['doc_id'])
                    break
                time.sleep(.1)
            else:
                raise RuntimeError(f'Modelica source did not parse: {path}')
        generation = execute('GetDocumentSource', doc_id=documents[0])['generation']
        # Admit the headless-safe solve first in a cold process. A preceding
        # workbench experiment must not hide missing library-root admission.
        physical = execute('RunModelicaSolve', doc_id=documents[0],
                           **{'class': 'ModelPhysicalContracts', 'source_generation': generation,
                              't_start': 0., 't_end': 3., 'dt': .01, 'label': 'Physical contracts'})
        if args.headless:
            generation = execute('GetDocumentSource', doc_id=documents[1])['generation']
            allocator = execute('RunModelicaSolve', doc_id=documents[1],
                                **{'class': 'WrenchAllocatorContract', 'source_generation': generation,
                                   't_start': 0., 't_end': .1, 'dt': .01, 'label': 'Allocation contracts'})
        else:
            allocator = execute('RunExperiment', doc_id=documents[1],
                                **{'class': 'WrenchAllocatorContract', 't_end': .1, 'dt': .01})
        runs = [physical['experiment_id'], allocator['experiment_id']]
        execute('SetTimeTransport', playing=True)
        run_authored_scenario(session, 'lunco://scenarios/tests/model_physical_contracts.rhai',
                              title='MODEL PHYSICAL CONTRACTS', check_count=44,
                              params={'contract_run': runs[0], 'allocator_run': runs[1]})
        for doc in documents:
            execute('CloseDocument', doc_id=doc)
    # The headless host persists its own experiment artifacts beside the
    # fixture. Retain only this run's records with the requested evidence.
    artifact_root = ROOT / 'assets/scenes/base/results'
    for run in runs:
        artifact = artifact_root / f'{run}.json'
        if artifact.is_file():
            shutil.move(str(artifact), output / artifact.name)
    if artifact_root.is_dir() and not any(artifact_root.iterdir()):
        artifact_root.rmdir()
    evidence = {'checks': 44, 'headless': args.headless, 'runs': runs,
                'port_released': not port_is_open(args.port)}
    (output / 'validation.json').write_text(json.dumps(evidence, indent=2))
    assert evidence['port_released']
    print('PASS 44 authored checks; owned API Exit and port release')


if __name__ == '__main__':
    main()
