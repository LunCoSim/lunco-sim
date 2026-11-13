#!/usr/bin/env python3
"""Allocate SemVer development releases and monotonic Windows MSI versions."""

from __future__ import annotations

import argparse
import re
import sys


_SEMVER = re.compile(
    r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    r"(?:-dev\.([1-9][0-9]*))?$"
)
_LEGACY_NIGHTLY = re.compile(
    r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    r"-nightly\.([1-9][0-9]*)\.([1-9][0-9]*)$"
)
_WINDOWS_PACKAGE = re.compile(
    r"^luncosim-(?:win-x64-)?(.+?)(?:-win-x64)?-full\.nupkg$",
    re.IGNORECASE,
)


def windows_installer_version(version: str) -> str:
    """Map app SemVer to the ordered three-number Windows Installer version."""
    match = _SEMVER.fullmatch(version)
    if match is None:
        raise ValueError(f"unsupported release version: {version}")
    major, minor, patch = map(int, match.groups()[:3])
    development = int(match.group(4)) if match.group(4) is not None else 999

    if major >= 255 or minor > 255:
        raise ValueError("release version exceeds the Windows Installer major/minor limits")
    if match.group(4) is not None and not 1 <= development <= 998:
        raise ValueError("development sequence must be 1..998; stable releases use slot 999")

    build = patch * 1000 + development
    if build > 65535:
        raise ValueError("release version exceeds the Windows Installer build limit")
    return f"{major + 1}.{minor}.{build}"


def _package_versions(asset_names: list[str]) -> list[str]:
    versions = []
    for name in asset_names:
        match = _WINDOWS_PACKAGE.fullmatch(name)
        if match is not None:
            versions.append(match.group(1))
    return versions


def next_development_version(base_version: str, asset_names: list[str]) -> str:
    """Allocate the next dev.N version for a Cargo base version."""
    if _SEMVER.fullmatch(base_version) is None or "-dev." in base_version:
        raise ValueError(f"invalid base SemVer version: {base_version}")

    major, minor, patch = base_version.split(".")
    base = (int(major), int(minor), int(patch))
    latest_dev = 0
    for version in _package_versions(asset_names):
        match = _SEMVER.fullmatch(version)
        if match is None:
            continue
        identity = tuple(map(int, match.groups()[:3]))
        if identity != base:
            continue
        if match.group(4) is None:
            raise ValueError(
                f"stable release {version} is already published; advance the Cargo base version"
            )
        latest_dev = max(latest_dev, int(match.group(4)))

    if latest_dev >= 998:
        raise ValueError("development release sequence exhausted; advance the Cargo base version")
    return f"{base_version}-dev.{latest_dev + 1}"


def _msi_identity(version: str) -> tuple[int, int, int]:
    match = _SEMVER.fullmatch(version)
    if match is not None:
        return tuple(map(int, windows_installer_version(version).split(".")))

    legacy = _LEGACY_NIGHTLY.fullmatch(version)
    if legacy is not None:
        run_number, attempt = map(int, legacy.groups()[3:])
        if not 1 <= attempt <= 99:
            raise ValueError(f"unsupported legacy nightly version: {version}")
        ordinal = run_number * 100 + attempt
        minor, build = divmod(ordinal, 65536)
        if minor > 255:
            raise ValueError("legacy MSI version sequence exhausted")
        return (1, minor, build)

    raise ValueError(f"unsupported published package version: {version}")


def check_published_versions(candidate: str, asset_names: list[str]) -> None:
    """Reject repeated dev versions and any MSI version that would move back."""
    proposed = _SEMVER.fullmatch(candidate)
    if proposed is None:
        raise ValueError(f"unsupported release version: {candidate}")
    proposed_identity = tuple(map(int, proposed.groups()[:3]))
    proposed_dev = int(proposed.group(4)) if proposed.group(4) is not None else None
    proposed_msi = _msi_identity(candidate)

    for published_version in _package_versions(asset_names):
        legacy = _LEGACY_NIGHTLY.fullmatch(published_version)
        if legacy is not None:
            published_msi = _msi_identity(published_version)
            if published_msi >= proposed_msi:
                raise ValueError(
                    f"MSI version {candidate} is not newer than published package {published_version}"
                )
            # `dev.N` starts a new user-facing version train. Legacy nightly
            # labels are not SemVer-compatible upgrade predecessors.
            continue

        match = _SEMVER.fullmatch(published_version)
        if match is None:
            continue
        published_identity = tuple(map(int, match.groups()[:3]))
        published_dev = int(match.group(4)) if match.group(4) is not None else None
        published_msi = _msi_identity(published_version)

        if published_msi >= proposed_msi:
            raise ValueError(
                f"MSI version {candidate} is not newer than published package {published_version}"
            )
        if published_identity > proposed_identity:
            raise ValueError(
                f"application version {candidate} is older than published package {published_version}"
            )
        if published_identity == proposed_identity:
            if published_dev is None or proposed_dev is None or published_dev >= proposed_dev:
                raise ValueError(
                    f"application version {candidate} is not newer than published package {published_version}"
                )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", help="print the MSI version mapped from app SemVer")
    parser.add_argument("--next-version", metavar="BASE", help="print the next base-dev.N version")
    parser.add_argument("--check-published", metavar="VERSION")
    args = parser.parse_args()

    modes = sum(value is not None for value in (args.version, args.next_version, args.check_published))
    if modes != 1:
        parser.error("specify exactly one of --version, --next-version, or --check-published")

    try:
        if args.version is not None:
            print(windows_installer_version(args.version))
        elif args.next_version is not None:
            print(next_development_version(args.next_version, sys.stdin.read().splitlines()))
        else:
            check_published_versions(args.check_published, sys.stdin.read().splitlines())
    except ValueError as error:
        parser.error(str(error))


if __name__ == "__main__":
    main()
