#!/usr/bin/env python3
"""Checks the bundled MCP extension's program in an installed or staged
RAWmakase: it is there and executable, built for the expected architecture,
links nothing but the system's libraries, and `rawmakase mcp` finds it and
completes an MCP initialize with it.

    python3 packaging/check_mcp.py RAWMAKASE PROGRAM [--arch ARCH] [--version VERSION]

RAWMAKASE is what users run (on Linux the usr/bin launcher), PROGRAM the
rawmakase-mcp beside the app. No running app is needed: initialize doesn't
reach it, and an empty data folder is used.
"""
import argparse
import json
import os
from pathlib import Path
import re
import struct
import subprocess
import sys
import tempfile

# The architecture names packaging uses, as one spelling each.
ARCHITECTURES = {"x86_64": "x86_64", "amd64": "x86_64", "x64": "x86_64",
                 "aarch64": "aarch64", "arm64": "aarch64"}
ELF_MACHINES = {62: "x86_64", 183: "aarch64"}
MACHO_CPUS = {0x01000007: "x86_64", 0x0100000C: "aarch64"}
PE_MACHINES = {0x8664: "x86_64", 0xAA64: "aarch64"}
# Linux libraries every distribution has; the program needs nothing more.
LINUX_SYSTEM = re.compile(r"^(linux-(vdso|gate)[\w.]*|ld-linux[-\w.]*|"
                          r"lib(c|m|dl|pthread|rt|util|gcc_s)\.so[.\d]*)$")


def architectures(data):
    """The architectures of the executable whose bytes are `data`."""
    if data[:4] == b"\x7fELF":
        order = "<" if data[5] == 1 else ">"
        (machine,) = struct.unpack_from(order + "H", data, 18)
        return [ELF_MACHINES.get(machine, f"elf-{machine}")]
    if data[:4] in (b"\xcf\xfa\xed\xfe", b"\xce\xfa\xed\xfe"):
        (cpu,) = struct.unpack_from("<i", data, 4)
        return [MACHO_CPUS.get(cpu & 0xFFFFFFFF, f"macho-{cpu:#x}")]
    if data[:4] == b"\xca\xfe\xba\xbe":
        (count,) = struct.unpack_from(">I", data, 4)
        cpus = [struct.unpack_from(">I", data, 8 + 20 * i)[0] for i in range(count)]
        return [MACHO_CPUS.get(cpu, f"macho-{cpu:#x}") for cpu in cpus]
    if data[:2] == b"MZ":
        (offset,) = struct.unpack_from("<I", data, 0x3C)
        if data[offset:offset + 4] == b"PE\0\0":
            (machine,) = struct.unpack_from("<H", data, offset + 4)
            return [PE_MACHINES.get(machine, f"pe-{machine:#x}")]
    raise SystemExit("not an executable this script reads")


def pe_imports(data):
    """The DLLs a PE executable imports, from its import directory."""
    (offset,) = struct.unpack_from("<I", data, 0x3C)
    sections, optional_size = struct.unpack_from("<H12xH", data, offset + 6)
    optional = offset + 24
    magic = struct.unpack_from("<H", data, optional)[0]
    directories = optional + (112 if magic == 0x20B else 96)
    import_rva, _ = struct.unpack_from("<II", data, directories + 8)
    table = []
    for i in range(sections):
        header = optional + optional_size + 40 * i
        size, address, _, raw = struct.unpack_from("<IIII", data, header + 8)
        table.append((address, size, raw))

    def file_offset(rva):
        for address, size, raw in table:
            if address <= rva < address + max(size, 1):
                return rva - address + raw
        raise SystemExit(f"import table address {rva:#x} is in no section")

    names = []
    entry = file_offset(import_rva) if import_rva else None
    while entry is not None:
        name_rva = struct.unpack_from("<I", data, entry + 12)[0]
        if name_rva == 0:
            break
        start = file_offset(name_rva)
        names.append(data[start:data.index(b"\0", start)].decode())
        entry += 20
    return names


def foreign_libraries(program, data):
    """The libraries `program` links that aren't the operating system's."""
    if data[:4] == b"\x7fELF":
        listing = subprocess.run(["ldd", str(program)], capture_output=True, text=True).stdout
        if "not found" in listing:
            return [listing]
        names = [line.split()[0] for line in listing.splitlines() if line.strip()]
        return [name for name in names if not LINUX_SYSTEM.match(Path(name).name)]
    if data[:2] == b"MZ":
        system = Path(os.environ.get("SystemRoot", r"C:\Windows")) / "System32"
        foreign = []
        for name in pe_imports(data):
            dll = name.lower()
            if dll.startswith(("api-ms-win-", "ext-ms-")):
                continue
            # A clean Windows has no Visual C++ runtime: the C runtime is
            # linked in.
            if re.match(r"^(vcruntime|msvcp|vcomp|concrt|ucrtbased)", dll) or not (system / dll).exists():
                foreign.append(name)
        return foreign
    listing = subprocess.run(["otool", "-L", str(program)], capture_output=True, text=True, check=True)
    paths = [line.strip().split(" (", 1)[0] for line in listing.stdout.splitlines()[1:]]
    return [path for path in paths if not path.startswith(("/usr/lib/", "/System/Library/"))]


def initialize(rawmakase, version):
    """`rawmakase mcp` in an empty data folder answers an MCP initialize."""
    request = {"jsonrpc": "2.0", "id": 1, "method": "initialize",
               "params": {"protocolVersion": "2025-11-25", "capabilities": {},
                          "clientInfo": {"name": "packaging-check", "version": "1"}}}
    with tempfile.TemporaryDirectory(prefix="rawmakase-mcp-check-") as data:
        environment = dict(os.environ, RAWMAKASE_DATA_DIR=data)
        for name in ("LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH", "DYLD_FALLBACK_LIBRARY_PATH"):
            environment.pop(name, None)
        done = subprocess.run([str(rawmakase), "mcp", "--data-dir", data],
                              input=json.dumps(request) + "\n", capture_output=True,
                              text=True, timeout=60, env=environment)
    if done.returncode != 0:
        raise SystemExit(f"rawmakase mcp exited with {done.returncode}: {done.stderr}")
    lines = done.stdout.splitlines()
    if not lines:
        raise SystemExit(f"rawmakase mcp answered nothing: {done.stderr}")
    server = json.loads(lines[0])["result"]["serverInfo"]
    if server["name"] != "rawmakase":
        raise SystemExit(f"unexpected MCP server {server}")
    if version and server["version"] != version:
        raise SystemExit(f"the MCP server reports {server['version']}, expected {version}")
    return server


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("rawmakase", type=Path)
    parser.add_argument("program", type=Path)
    parser.add_argument("--arch", help="x86_64, aarch64 (or amd64, arm64)")
    parser.add_argument("--version", help="the version the MCP server must report")
    args = parser.parse_args()
    if not args.program.is_file():
        raise SystemExit(f"{args.program} is missing")
    if os.name != "nt" and not os.access(args.program, os.X_OK):
        raise SystemExit(f"{args.program} is not executable")
    data = args.program.read_bytes()
    found = architectures(data)
    if args.arch:
        expected = ARCHITECTURES.get(args.arch.lower(), args.arch)
        if found != [expected]:
            raise SystemExit(f"{args.program} is built for {found}, expected {expected}")
    foreign = foreign_libraries(args.program, data)
    if foreign:
        raise SystemExit(f"{args.program} links libraries that aren't the system's: {foreign}")
    server = initialize(args.rawmakase, args.version)
    print(f"{args.program}: {', '.join(found)}, system libraries only; "
          f"rawmakase mcp initialized {server['name']} {server['version']}")


if __name__ == "__main__":
    sys.exit(main())
