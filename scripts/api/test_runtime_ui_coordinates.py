#!/usr/bin/env python3
"""Run the authored coordinate presentation gate on an owned production API."""
import argparse
from runtime import ROOT, request_json

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--port", type=int, required=True)
args = parser.parse_args()
code = (ROOT / "assets/scripting/policy/runtime_ui_properties.rhai").read_text()
code += "\n" + (ROOT / "assets/scripting/tests/test_runtime_ui_coordinates.rhai").read_text()
result = request_json(args.port, dict(type="ExecuteCommand", command="RunRhai", params=dict(code=code)))
output = result.get("data", {}).get("stdout", "")
print(output)
assert "TESTS_OK 7" in output and "TESTS_FAIL" not in output, result
