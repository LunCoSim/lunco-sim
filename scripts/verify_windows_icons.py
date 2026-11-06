#!/usr/bin/env python3
"""Check that a packaged Windows executable and launcher contain its ICO frames."""

import ctypes
from ctypes import wintypes
from pathlib import Path
import struct
import sys
import tempfile
import zipfile


def ico_frames(path: Path) -> list[bytes]:
    data = path.read_bytes()
    reserved, kind, count = struct.unpack_from("<HHH", data)
    if reserved != 0 or kind != 1 or count == 0:
        raise ValueError(f"Invalid ICO: {path}")
    frames = []
    for index in range(count):
        size, offset = struct.unpack_from("<II", data, 6 + 16 * index + 8)
        frame = data[offset : offset + size]
        if len(frame) != size:
            raise ValueError(f"Truncated ICO: {path}")
        frames.append(frame)
    return frames


def resource_frames(path: Path) -> list[bytes]:
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.LoadLibraryExW.argtypes = [wintypes.LPCWSTR, wintypes.HANDLE, wintypes.DWORD]
    kernel.LoadLibraryExW.restype = wintypes.HANDLE
    kernel.FindResourceW.argtypes = [wintypes.HANDLE, ctypes.c_void_p, ctypes.c_void_p]
    kernel.FindResourceW.restype = wintypes.HANDLE
    kernel.LoadResource.argtypes = [wintypes.HANDLE, wintypes.HANDLE]
    kernel.LoadResource.restype = wintypes.HANDLE
    kernel.SizeofResource.argtypes = [wintypes.HANDLE, wintypes.HANDLE]
    kernel.SizeofResource.restype = wintypes.DWORD
    kernel.LockResource.argtypes = [wintypes.HANDLE]
    kernel.LockResource.restype = ctypes.c_void_p
    kernel.FreeLibrary.argtypes = [wintypes.HANDLE]

    module = kernel.LoadLibraryExW(str(path.resolve()), None, 0x2)
    if not module:
        raise ctypes.WinError(ctypes.get_last_error())

    def read_resource(name: int, kind: int) -> bytes:
        resource = kernel.FindResourceW(module, name, kind)
        if not resource:
            raise ValueError(f"{path} is missing icon resource {kind}:{name}")
        size = kernel.SizeofResource(module, resource)
        loaded = kernel.LoadResource(module, resource)
        address = kernel.LockResource(loaded) if loaded else None
        if not size or not address:
            raise ValueError(f"{path} has an unreadable icon resource {kind}:{name}")
        return ctypes.string_at(address, size)

    try:
        group = read_resource(1, 14)  # RT_GROUP_ICON
        reserved, kind, count = struct.unpack_from("<HHH", group)
        if reserved != 0 or kind != 1 or count == 0:
            raise ValueError(f"{path} has an invalid icon group")
        frames = []
        for index in range(count):
            size, resource_id = struct.unpack_from("<IH", group, 6 + 14 * index + 8)
            frame = read_resource(resource_id, 3)  # RT_ICON
            if len(frame) != size:
                raise ValueError(f"{path} has a truncated icon frame")
            frames.append(frame)
        return frames
    finally:
        kernel.FreeLibrary(module)


def main() -> None:
    if sys.platform != "win32" or len(sys.argv) != 4:
        raise SystemExit("usage on Windows: verify_windows_icons.py <exe> <ico> <portable.zip>")
    executable, icon, portable = map(Path, sys.argv[1:])
    expected = ico_frames(icon)
    if resource_frames(executable) != expected:
        raise ValueError(f"{executable} does not contain the generated application icon")
    with zipfile.ZipFile(portable) as archive, tempfile.TemporaryDirectory() as directory:
        launcher = Path(directory) / "LunCoSim.exe"
        launcher.write_bytes(archive.read("LunCoSim.exe"))
        if resource_frames(launcher) != expected:
            raise ValueError(f"{portable} launcher does not contain the generated application icon")
    print(f"Verified Windows executable and launcher icons: {portable.name}")


if __name__ == "__main__":
    main()
