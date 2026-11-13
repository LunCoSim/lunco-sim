#!/usr/bin/env python3
"""Map nightly workflow runs to an ordered Windows Installer version."""

import argparse
import re
import sys


def nightly_msi_version(run_number: int, attempt: int) -> str:
    """Give each run and retry a distinct, ordered MSI version."""
    if run_number < 1 or not 1 <= attempt <= 99:
        raise ValueError("run number must be positive and attempt must be 1..99")
    ordinal = run_number * 100 + attempt
    minor, build = divmod(ordinal, 65536)
    if minor > 255:
        raise ValueError("MSI version sequence exhausted; allocate a new major version")
    return f"1.{minor}.{build}"


_NIGHTLY = re.compile(
    r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    r"-nightly\.([1-9][0-9]*)\.([1-9][0-9]*)$"
)


def nightly_identity(version: str) -> tuple[int, int, int, int, int]:
    match = _NIGHTLY.fullmatch(version)
    if match is None:
        raise ValueError(f"unsupported nightly version: {version}")
    identity = tuple(map(int, match.groups()))
    nightly_msi_version(identity[3], identity[4])
    return identity


def check_published_versions(candidate: str, asset_names: list[str]) -> None:
    """Reject out-of-order publication in both MSI and app version order."""
    proposed = nightly_identity(candidate)
    proposed_sequence = proposed[3] * 100 + proposed[4]
    suffix = "-win-x64-full.nupkg"
    for name in asset_names:
        if not name.startswith("luncosim-") or not name.endswith(suffix):
            continue
        published = nightly_identity(name[len("luncosim-") : -len(suffix)])
        if published[3] * 100 + published[4] >= proposed_sequence:
            raise ValueError(f"MSI sequence is not newer than published package {name}")
        if published >= proposed:
            raise ValueError(f"application version is not newer than published package {name}")


def next_nightly_run_number(asset_names: list[str]) -> int:
    """Continue the existing version sequence after moving the workflow repo."""
    suffix = "-win-x64-full.nupkg"
    latest_run_number = 0
    for name in asset_names:
        if not name.startswith("luncosim-") or not name.endswith(suffix):
            continue
        version = name[len("luncosim-") : -len(suffix)]
        if "-nightly." not in version:
            continue
        latest_run_number = max(latest_run_number, nightly_identity(version)[3])
    return latest_run_number + 1


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run_number", type=int, nargs="?")
    parser.add_argument("attempt", type=int, nargs="?")
    parser.add_argument("--check-published", metavar="VERSION")
    parser.add_argument("--next-run", action="store_true")
    args = parser.parse_args()
    try:
        if args.next_run:
            if (
                args.check_published is not None
                or args.run_number is not None
                or args.attempt is not None
            ):
                parser.error("--next-run cannot be combined with other arguments")
            print(next_nightly_run_number(sys.stdin.read().splitlines()))
        elif args.check_published is not None:
            if args.run_number is not None or args.attempt is not None:
                parser.error("--check-published cannot be combined with run numbers")
            check_published_versions(args.check_published, sys.stdin.read().splitlines())
        else:
            if args.run_number is None or args.attempt is None:
                parser.error("run number and attempt are required")
            print(nightly_msi_version(args.run_number, args.attempt))
    except ValueError as error:
        parser.error(str(error))


if __name__ == "__main__":
    main()
