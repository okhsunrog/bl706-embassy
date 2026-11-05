"""Inspect vendor BLE archives without linking or executing their contents.

Requires GNU ar, nm and readelf. Run with `uv run --no-project`.
Section totals describe all archive members before linker garbage collection;
they are not the flash/RAM footprint of a linked application.
"""

import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess


def command(*args):
    return subprocess.run(
        args, check=True, text=True, capture_output=True
    ).stdout


def defined_symbols(path):
    result = set()
    for line in command("nm", "-g", "--defined-only", str(path)).splitlines():
        fields = line.split()
        if len(fields) == 3:
            result.add(fields[2])
    return result


def undefined_symbols(path):
    members = {}
    member = None
    for line in command("nm", "-u", str(path)).splitlines():
        if line.endswith(":"):
            member = line[:-1]
            members.setdefault(member, set())
            continue
        fields = line.split()
        if len(fields) == 2 and fields[0] == "U":
            if member is None:
                raise ValueError("nm output lacks an archive member header")
            members[member].add(fields[1])
    return members


SECTION = re.compile(
    r"^\s*\[\s*\d+\]\s+(\S+)\s+(\S+)\s+[0-9a-fA-F]+"
    r"\s+[0-9a-fA-F]+\s+([0-9a-fA-F]+)\s+[0-9a-fA-F]+"
    r"\s+([A-Z]*)\s+\d+\s+\d+\s+\d+\s*$"
)
RTOS = re.compile(
    r"^(?:xQueue|xTask|vTask|vQueue|uxTask|pvPort|vPort|xSemaphore|xTimer|aos_|krhino)"
)
ENTRYPOINTS = (
    "ble_controller_init",
    "ble_controller_task_init",
    "blecontroller_main",
    "rwip_schedule",
    "hci_send_2_controller",
    "bt_onchiphci_interface_init",
    "bt_onchiphci_send",
    "bt_onchiphci_hanlde_rx_acl",
)


def audit(path):
    if not path.read_bytes().startswith(b"!<arch>\n"):
        raise ValueError(f"not a regular ar archive: {path}")
    defined = defined_symbols(path)
    undefined = undefined_symbols(path)
    external = set().union(*undefined.values()) - defined
    rtos = sorted(s for s in external if RTOS.match(s))
    sections = []
    for line in command("readelf", "-SW", str(path)).splitlines():
        match = SECTION.match(line)
        if match:
            name, kind, size, flags = match.groups()
            sections.append((name, kind, int(size, 16), flags))
    if not sections:
        raise ValueError(f"no ELF sections parsed: {path}")
    attributes = command("readelf", "-A", str(path))
    headers = command("readelf", "-h", str(path))
    return {
        "archive": str(path),
        "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        "archive_bytes": path.stat().st_size,
        "member_count": len(command("ar", "t", str(path)).splitlines()),
        "riscv_arch": sorted(set(re.findall(r'Tag_RISCV_arch: "([^"]+)"', attributes))),
        "elf_flags": sorted(set(re.findall(r"Flags:\s+(.+)", headers))),
        "debug_info_members": sum(s[0] == ".debug_info" for s in sections),
        "debug_info_bytes": sum(s[2] for s in sections if s[0] == ".debug_info"),
        "all_members_alloc_progbits_bytes": sum(
            s[2] for s in sections if s[1] == "PROGBITS" and "A" in s[3]
        ),
        "all_members_alloc_nobits_bytes": sum(
            s[2] for s in sections if s[1] == "NOBITS" and "A" in s[3]
        ),
        "defined_global_count": len(defined),
        "available_entrypoints": [s for s in ENTRYPOINTS if s in defined],
        "strong_external_symbols": sorted(external),
        "rtos_imports": rtos,
        "rtos_importing_members": {
            name: sorted(symbols & set(rtos))
            for name, symbols in undefined.items()
            if symbols & set(rtos)
        },
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", nargs="+", type=Path)
    args = parser.parse_args()
    print(json.dumps([audit(path) for path in args.archive], indent=2))


if __name__ == "__main__":
    main()
