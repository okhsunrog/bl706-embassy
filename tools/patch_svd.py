"""Auditable, dependency-free corrections. Run with uv run, never edit vendor input.

See docs/pac-audit.md for evidence and intentionally unreviewed peripherals.
"""
import copy
import hashlib
from pathlib import Path
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "svd/vendor/soc702_reg.svd"
assert hashlib.sha256(SOURCE.read_bytes()).hexdigest() == "6e623b60330411b91ac1161cb7414268f622dc282b79705a05b55a2fe44d936b"
tree = ET.parse(SOURCE)
device = tree.getroot()
device.find("name").text = "BL706"
ET.SubElement(device, "version").text = "1.0"
# Vendor's incomplete <cpu> is not CMSIS-SVD-valid and is not an ISA authority.
device.remove(device.find("cpu"))
peripherals = device.find("peripherals")
by_name = {p.findtext("name").lower(): p for p in peripherals}

def reg(p, name):
    return next(r for r in by_name[p].find("registers") if r.findtext("name") == name)

def set_text(e, tag, value):
    c = e.find(tag)
    if c is None:
        c = ET.SubElement(e, tag)
    c.text = value

# SDK bl702.h and RM memory map. andelf retains the two incorrect bases.
for p, address in [("aon", "0x4000F000"), ("dma", "0x4000C000"), ("kys", "0x4000A900")]:
    by_name[p].find("baseAddress").text = address
for name in ["hbn", "aon"]:
    by_name[name].find("name").text = name
    by_name[name].find("groupName").text = name

# The source has only the first instance of these identical register blocks.
for src, name, address in [("uart", "uart1", "0x4000A100"),
                            ("qdec", "qdec1", "0x4000A840"),
                            ("qdec", "qdec2", "0x4000A880")]:
    p = copy.deepcopy(by_name[src])
    p.find("name").text = name
    p.find("baseAddress").text = address
    peripherals.append(p)
    by_name[name] = p
by_name["uart"].find("name").text = "uart0"

# Only named gates checked against GLB_AHB_{Get,UnGate}_Clock are split.
# andelf's KYS=25 and MAX=31 are not copied: SDK uses 27 for KYS.
fields = reg("glb", "cgen_cfg1").find("fields")
fields.clear()
for name, bit, width in [("cgen_s1", 0, 16), ("uart0", 16, 1), ("uart1", 17, 1),
                          ("spi", 18, 1), ("i2c", 19, 1), ("pwm", 20, 1),
                          ("timer", 21, 1), ("ir", 22, 1), ("cgen_s1a_high", 23, 9)]:
    f = ET.SubElement(fields, "field")
    for tag, value in [("name", name), ("bitOffset", str(bit)), ("bitWidth", str(width))]:
        ET.SubElement(f, tag).text = value

# Register-level access for the HAL's reviewed I/O. Mixed status/control regs
# remain RW: W1C handling must use explicit writes, never generic RMW.
for p, names, access in [
    ("glb", ["GPIO_CFGCTL30", "GPIO_INT_STAT1"], "read-only"),
    ("glb", ["GPIO_INT_CLR1"], "write-only"),
    ("i2c", ["i2c_fifo_rdata"], "read-only"),
    ("i2c", ["i2c_fifo_wdata"], "write-only"),
    ("uart", ["uart_fifo_rdata", "uart_status"], "read-only"),
    ("uart", ["uart_fifo_wdata", "uart_int_clear"], "write-only"),
]:
    for name in names:
        set_text(reg(p, name), "access", access)
        for f in reg(p, name).findall("./fields/field"):
            # Don't let an inherited field access contradict register access.
            if f.find("access") is not None:
                f.remove(f.find("access"))
# Keep UART1's access annotations identical to UART0.
by_name["uart1"].remove(by_name["uart1"].find("registers"))
by_name["uart1"].append(copy.deepcopy(by_name["uart"].find("registers")))

# SDK IRQn = peripheral source + 16. Core-local MTIME is handled by HAL.
for p, name, number in [("uart", "UART0", 45), ("uart1", "UART1", 46), ("i2c", "I2C", 48), ("glb", "GPIO", 60)]:
    irq = ET.SubElement(by_name[p], "interrupt")
    ET.SubElement(irq, "name").text = name
    ET.SubElement(irq, "value").text = str(number)

ET.indent(tree)
tree.write(ROOT / "svd/patched.svd", encoding="utf-8", xml_declaration=True)
