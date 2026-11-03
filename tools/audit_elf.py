"""Offline ELF32 checks; no target execution. uv run --no-project tools/audit_elf.py.

Scan instruction encodings, not just objdump text: unsupported opcodes can be
printed as <unknown> by disassemblers without +a. Also checks compressed FP ops.
"""
from pathlib import Path
import struct
import sys

ROOT = Path(__file__).resolve().parents[1]

def audit(path, fpu=False, probes=False):
    data = path.read_bytes()
    assert data[:7] == b"\x7fELF\x01\x01\x01", path
    h = struct.unpack_from("<16sHHIIIIIHHHHHH", data)
    _, _, machine, _, entry, phoff, shoff, flags, _, phsize, phnum, shsize, shnum, shstrndx = h
    assert machine == 243
    assert flags & 6 == (2 if fpu else 0), (path, "float ABI", flags)
    headers = [struct.unpack_from("<10I", data, shoff+i*shsize) for i in range(shnum)]
    strs = headers[shstrndx]
    strings = data[strs[4]:strs[4]+strs[5]]
    def name(table, offset):
        return table[offset:table.index(b"\0", offset)].decode()
    sections = {name(strings, sh[0]): sh for sh in headers}
    syms = {}
    for sh in headers:
        if sh[1] != 2:
            continue
        names = headers[sh[6]]
        table = data[names[4]:names[4]+names[5]]
        for pos in range(sh[4], sh[4]+sh[5], sh[9]):
            n, value, *_ = struct.unpack_from("<IIIBBH", data, pos)
            syms[name(table, n)] = value
    assert entry == syms["_start"] == 0x22010000
    assert syms["bl706_vectors"] % 64 == 0
    assert syms["bl706_fatal_trap"] % 64 == 0
    assert syms["__bss_end"] <= syms["__stack_bottom"]
    assert syms["__stack_top"] - syms["__stack_bottom"] == 8192
    assert syms["__stack_top"] <= 0x2202c000
    # Every load segment has VMA=LMA and stays in explicitly reserved RAM.
    for i in range(phnum):
        kind, _, va, pa, _, memsz, _, _ = struct.unpack_from("<8I", data, phoff+i*phsize)
        if kind != 1 or not memsz:
            continue
        assert va == pa
        assert any(lo <= va and va+memsz <= hi for lo, hi in [(0x22010000,0x22014000),(0x22020000,0x2202c000)]), (path, hex(va), memsz)
    sh = sections[".vectors"]
    vectors = struct.unpack_from("<80I", data, sh[4])
    assert sh[5] == 320
    assert vectors[7] == vectors[48] == syms["bl706_irq_entry"]
    assert all(v == syms["bl706_fatal_trap"] for i,v in enumerate(vectors) if i not in (7,48))
    atomic = []
    float_ops = 0
    saved, restored = set(), set()
    for sh in headers:
        if not sh[2] & 4:
            continue
        pos, end = sh[4], sh[4]+sh[5]
        while pos < end:
            half = struct.unpack_from("<H", data, pos)[0]
            pc = sh[3] + pos - sh[4]
            if half & 3 != 3:
                q, funct = half & 3, half >> 13
                if q in (0,2) and funct in (1,3,5,7):
                    float_ops += 1
                if q == 2 and funct == 7: saved.add((half >> 2) & 31)
                if q == 2 and funct == 3: restored.add((half >> 7) & 31)
                pos += 2
            else:
                assert half & 0x1f != 0x1f, "unexpected >32-bit encoding"
                word = struct.unpack_from("<I", data, pos)[0]
                opcode = word & 0x7f
                if opcode == 0x2f: atomic.append(pc)
                if opcode in (0x07,0x27,0x43,0x47,0x4b,0x4f,0x53): float_ops += 1
                if (word >> 15) & 31 == 2 and (word >> 12) & 7 == 2:
                    if opcode == 0x27: saved.add((word >> 20) & 31)
                    if opcode == 0x07: restored.add((word >> 7) & 31)
                pos += 4
    expected = {syms[n] for n in ("bl706_probe_lr", "bl706_probe_sc", "bl706_probe_amo_insn")} if probes else set()
    assert set(atomic) == expected and len(atomic) == len(expected), (path, "unexpected A opcodes", atomic)
    if fpu:
        assert float_ops > 0
        assert saved == restored == set(range(32)), (path, "FP context save/restore incomplete", saved, restored)
    else:
        assert float_ops == 0, (path, "unexpected FP opcode", float_ops)
    print(f"{path.relative_to(ROOT)}: RAM/vector/ABI checks OK; A={len(atomic)}, FP={float_ops}, stack=8192")

if __name__ == "__main__":
    for target, fpu, names in [
        ("riscv32imc-unknown-none-elf", False, ["heartbeat", "fusb302"]),
        ("riscv32imfc-unknown-none-elf", True, ["heartbeat", "fusb302", "fpu"]),
    ]:
        for binary in names:
            audit(ROOT/"target"/target/"release"/binary, fpu=fpu)
    audit(ROOT/"target/riscv32imc-unknown-none-elf/release/atomics", probes=True)
