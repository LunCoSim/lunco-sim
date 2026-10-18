"""Shared production-runtime lifecycle for API acceptance scripts.

The scripts intentionally invoke the already-built production binary directly.
They never hide a Cargo rebuild, and they only consider a session usable after
the runtime readiness contract reports a ready world.
"""

from __future__ import annotations

import json
import os
import re
import socket
import subprocess
import time
from pathlib import Path
from typing import Any, Iterable
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen


ROOT = Path(__file__).resolve().parents[2]
_binary = Path(os.environ.get("LUNCOSIM_BIN", "target/debug/luncosim"))
BINARY = _binary if _binary.is_absolute() else ROOT / _binary
REQUEST_TIMEOUT_S = float(os.environ.get("LUNCOSIM_API_REQUEST_TIMEOUT_S", "10"))
READY_TIMEOUT_S = float(os.environ.get("LUNCOSIM_API_READY_TIMEOUT_S", "120"))
EXIT_TIMEOUT_S = float(os.environ.get("LUNCOSIM_API_EXIT_TIMEOUT_S", "15"))
POLL_INTERVAL_S = float(os.environ.get("LUNCOSIM_API_POLL_INTERVAL_S", "0.25"))


class RuntimeErrorWithLog(RuntimeError):
    """An API lifecycle failure that includes the captured process log path."""


def assert_render_log_clean(path: Path) -> None:
    """GPU callback failures are not part of the domain diagnostics query."""
    text = path.read_text(encoding="utf-8", errors="replace")
    failure = re.search(
        r"wgpu validation error|wgpu device lost|failed to process shader|"
        r"failed to create shader module|Encountered a panic|thread .*panicked",
        text, re.IGNORECASE,
    )
    if failure:
        raise RuntimeErrorWithLog(f"production render failed: {text[failure.start():failure.start() + 500]}; log={path}")


def _json_response(response: Any) -> dict[str, Any]:
    return json.loads(response.read().decode("utf-8"))


def request_json(port: int, payload: dict[str, Any], timeout_s: float = REQUEST_TIMEOUT_S) -> dict[str, Any]:
    """POST one tagged API request and decode its JSON response."""
    request = Request(
        f"http://127.0.0.1:{port}/api/commands",
        data=json.dumps(payload).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urlopen(request, timeout=timeout_s) as response:
            return _json_response(response)
    except HTTPError as error:
        body = error.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"API request returned HTTP {error.code}: {body}") from error
    except (URLError, TimeoutError) as error:
        raise RuntimeError(f"API request failed on port {port}: {error}") from error


def get_json(port: int, path: str, timeout_s: float = REQUEST_TIMEOUT_S) -> dict[str, Any]:
    """GET one API transport endpoint and decode its JSON response."""
    try:
        with urlopen(f"http://127.0.0.1:{port}{path}", timeout=timeout_s) as response:
            return _json_response(response)
    except (HTTPError, URLError, TimeoutError, ConnectionError) as error:
        raise RuntimeError(f"API GET {path} failed on port {port}: {error}") from error


def _readiness_is_clear(response: dict[str, Any]) -> bool:
    data = response.get("data")
    return isinstance(data, dict) and data.get("ready") is True and data.get("world_hold") is False


def wait_for_ready(port: int, timeout_s: float = READY_TIMEOUT_S) -> dict[str, Any]:
    """Wait until `/api/ready` reports a fully usable world."""
    deadline = time.monotonic() + timeout_s
    last_response: dict[str, Any] | None = None
    while time.monotonic() < deadline:
        try:
            last_response = get_json(port, "/api/ready")
            if _readiness_is_clear(last_response):
                return last_response
        except RuntimeError:
            pass
        time.sleep(POLL_INTERVAL_S)
    raise RuntimeError(
        f"runtime on port {port} did not become ready within {timeout_s:g}s; "
        f"last response: {last_response}"
    )


def port_is_open(port: int) -> bool:
    """Return whether the local API port still accepts TCP connections."""
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=REQUEST_TIMEOUT_S):
            return True
    except OSError:
        return False


def run_authored_scenario(
    session: "ProductionSession",
    source_asset: str,
    *,
    title: str,
    check_count: int,
    params: dict[str, Any] | None = None,
    allow_runtime_fault: bool = False,
    timeout_s: float = 120,
) -> None:
    """Require a retained observer's own verdict, including post-fault checks.

    Retention keeps the observer alive across a scene projection transaction.
    Allowing a fault never counts as success: the observer must still publish
    its named verdict and exact assertion count after admission.
    """
    offset = session.log_path.stat().st_size
    response = session.post({"type": "ExecuteCommand", "command": "RunScenarioAsset",
                             "params": {"source_asset": source_asset,
                                        "reload_policy": "Retain", "params": params or {}}})
    if response.get("error"):
        raise RuntimeError(f"scenario admission failed: {response}")
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        with session.log_path.open("rb") as log:
            log.seek(offset)
            output = log.read().decode("utf-8", errors="replace")
        if any(marker in output for marker in (
            "[rhai] TESTS_FAIL", "failed to load requested scenario asset", "[rhai] task leaf failed:",
        )):
            raise RuntimeErrorWithLog(f"authored scenario failed: {source_asset}; log={session.log_path}")
        if f"[rhai] {title}: PASS" in output and re.search(
            rf"\[rhai\] TESTS_OK {check_count}(?:\r?\n|$)", output
        ):
            print(f"{title}: PASS ({check_count} authored checks)", flush=True)
            return
        readiness = session.post({"type": "ExecuteCommand", "command": "GetReadiness", "params": {}})
        if readiness.get("error"):
            raise RuntimeError(f"scenario readiness failed: {readiness}")
        if readiness.get("data", {}).get("faulted") and not allow_runtime_fault:
            raise RuntimeErrorWithLog(f"scenario runtime fault: {readiness}; log={session.log_path}")
        if session.process is None or session.process.poll() is not None:
            raise RuntimeErrorWithLog(f"runtime exited before authored verdict; log={session.log_path}")
        time.sleep(POLL_INTERVAL_S)
    raise RuntimeErrorWithLog(f"authored scenario timed out: {source_asset}; log={session.log_path}")


class ProductionSession:
    """Run one production `luncosim` API session with verified shutdown."""

    def __init__(
        self,
        port: int,
        *,
        extra_args: Iterable[str] = (),
        log_path: Path | None = None,
        windowed: bool = False,
        subcommand: str | None = None,
        require_readiness: bool = True,
    ) -> None:
        self.port = port
        self.extra_args = list(extra_args)
        self.log_path = log_path or Path(f"/tmp/luncosim-api-{port}.log")
        self.windowed = windowed
        self.subcommand = subcommand
        self.require_readiness = require_readiness
        self._log = None
        self.process: subprocess.Popen[bytes] | None = None

    def __enter__(self) -> "ProductionSession":
        if port_is_open(self.port):
            raise RuntimeErrorWithLog(f"API port {self.port} is already in use")
        if not BINARY.is_file():
            raise RuntimeErrorWithLog(
                f"missing production binary {BINARY}; build it first with "
                "`RUSTC_WRAPPER= cargo build -p lunco-luncosim --bin luncosim`"
            )
        self.log_path.parent.mkdir(parents=True, exist_ok=True)
        self._log = self.log_path.open("wb")
        args = [str(BINARY)]
        if self.subcommand is not None:
            args.append(self.subcommand)
        if not self.windowed:
            args.append("--no-ui")
        args.extend(["--api", str(self.port), *self.extra_args])
        self.process = subprocess.Popen(
            args,
            cwd=ROOT,
            stdin=subprocess.DEVNULL,
            stdout=self._log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        try:
            if self.require_readiness:
                wait_for_ready(self.port)
        except Exception as error:
            self._force_reap()
            if self._log is not None:
                self._log.close()
                self._log = None
            raise RuntimeErrorWithLog(f"{error}; process log: {self.log_path}") from error
        return self

    def post(self, payload: dict[str, Any]) -> dict[str, Any]:
        return request_json(self.port, payload)

    def capture_screenshot(self, path: Path, timeout_s: float = 10) -> None:
        """Require a newly published capture before shutting down this session."""
        path = path.resolve()
        path.parent.mkdir(parents=True, exist_ok=True)
        started = time.time_ns()
        response = self.post({"type": "ExecuteCommand", "command": "CaptureScreenshot",
                              "params": {"save_to_file": True, "path": str(path)}})
        if response.get("error"):
            raise RuntimeError(f"CaptureScreenshot: {response}")
        deadline = time.monotonic() + timeout_s
        while time.monotonic() < deadline:
            if path.is_file() and path.stat().st_mtime_ns >= started:
                return
            time.sleep(.05)
        raise RuntimeError(f"Screenshot was not published: {path}")

    def close(self) -> None:
        if self.process is None:
            return
        error: Exception | None = None
        try:
            if self.process.poll() is None:
                response = self.post(
                    {"type": "ExecuteCommand", "command": "Exit", "params": {}}
                )
                if response.get("error"):
                    raise RuntimeError(f"Exit command failed: {response}")
                self.process.wait(timeout=EXIT_TIMEOUT_S)
            if self.process.poll() is None:
                raise RuntimeError(f"process did not exit within {EXIT_TIMEOUT_S:g}s")
            deadline = time.monotonic() + EXIT_TIMEOUT_S
            while port_is_open(self.port) and time.monotonic() < deadline:
                time.sleep(POLL_INTERVAL_S)
            if port_is_open(self.port):
                raise RuntimeError(f"API port {self.port} remained open after Exit")
        except Exception as shutdown_error:  # cleanup must not leak a process
            error = shutdown_error
            self._force_reap()
        finally:
            if self._log is not None:
                self._log.close()
                self._log = None
        self.process = None
        if error is not None:
            raise RuntimeErrorWithLog(f"{error}; process log: {self.log_path}") from error

    def _force_reap(self) -> None:
        if self.process is not None and self.process.poll() is None:
            self.process.kill()
            self.process.wait()

    def __exit__(self, exc_type: Any, exc_value: Any, traceback: Any) -> bool:
        self.close()
        return False
