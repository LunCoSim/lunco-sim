#!/usr/bin/env python3
"""Evaluate a rhai snippet against a running sandbox and print its stdout.

`RunRhai` is deferred: the HTTP API holds the original request until the snippet
runs and returns its captured `print(...)` output in that response. This helper
writes the returned stdout to our stdout, so a shell harness can assert on it.
Stdlib only (mirrors the other scripts/api/*.py tools).

Usage:
    rhai_eval.py <port> -e '<rhai code>'
    rhai_eval.py <port> -f <path/to/script.rhai>
    rhai_eval.py <port> -e '<prelude>' -f <script.rhai>   # prelude prepended

Giving both `-e` and `-f` concatenates them (prelude first) — lets a harness
inject parameters (e.g. `let ASSERT_PRIMS = [...]`) ahead of a committed script.

Exit codes: 0 = got stdout; 2 = submit failed; 4 = rhai error.
"""
import json
import sys
import urllib.error
import urllib.request


def _post(port, obj):
    data = json.dumps(obj).encode()
    req = urllib.request.Request(
        f"http://127.0.0.1:{port}/api/commands",
        data=data,
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=5) as resp:
        return json.loads(resp.read().decode())


def rhai_eval(port, code):
    try:
        resp = _post(
            port,
            {
                "type": "ExecuteCommand",
                "command": "RunRhai",
                "params": {"code": code},
            },
        )
    except (urllib.error.HTTPError, urllib.error.URLError) as exc:
        print(f"ERROR: RunRhai request failed: {exc}", file=sys.stderr)
        return 2, ""

    if "error" in resp:
        print(f"RHAI_ERROR: {resp['error']}", file=sys.stderr)
        return 4, ""
    stdout = (resp.get("data") or {}).get("stdout")
    if stdout is None:
        print(f"ERROR: RunRhai returned no stdout: {resp}", file=sys.stderr)
        return 2, ""
    return 0, stdout


def main():
    if len(sys.argv) < 4:
        print(__doc__)
        sys.exit(2)
    port = int(sys.argv[1])
    # Parse -e <code> and/or -f <file> in any order; concatenate prelude + file.
    args = sys.argv[2:]
    prelude = ""
    body = ""
    i = 0
    while i < len(args):
        if args[i] == "-e" and i + 1 < len(args):
            prelude = args[i + 1]
            i += 2
        elif args[i] == "-f" and i + 1 < len(args):
            with open(args[i + 1]) as f:
                body = f.read()
            i += 2
        else:
            print(__doc__)
            sys.exit(2)
    code = (prelude + "\n" + body) if (prelude and body) else (prelude or body)
    if not code:
        print(__doc__)
        sys.exit(2)
    rc, out = rhai_eval(port, code)
    if out:
        sys.stdout.write(out if out.endswith("\n") else out + "\n")
    sys.exit(rc)


if __name__ == "__main__":
    main()
