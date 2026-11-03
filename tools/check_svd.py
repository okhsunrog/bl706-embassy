"""Normalize the three source SVDs and report semantic differences offline."""
from pathlib import Path
import json
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
NUMERIC = {"baseAddress", "addressOffset", "offset", "size", "lsb", "msb", "bitOffset", "bitWidth", "resetValue", "resetMask"}
def flatten(e, prefix=""):
    out = {}
    for c in e:
        key = c.tag + ("["+c.findtext("name")+"]" if c.findtext("name") else "")
        path = prefix+"/"+key
        if len(c):
            out.update(flatten(c,path))
        elif c.text and c.text.strip():
            value = c.text.strip()
            if c.tag in NUMERIC:
                value = int(value, 0)
            out[path] = value
    return out
def compare(a,b):
    x,y = flatten(ET.parse(a).getroot()),flatten(ET.parse(b).getroot())
    return [{"path":k,"before":x.get(k),"after":y.get(k)} for k in sorted(x.keys()|y.keys()) if x.get(k)!=y.get(k)]

official = ROOT/"svd/vendor/soc702_reg.svd"
andelf = ROOT/"svd/vendor/andelf.svd"
patched = ROOT/"svd/vendor/andelf.svd.patched"
changes = compare(official,andelf)
rf = [c for c in changes if "/peripheral[rf]/" in c["path"]]
assert all(c["before"] is None for c in rf)
report = {
    "official_to_andelf": {
        "added_rf_property_count": len(rf),
        "other_changes": [c for c in changes if c not in rf],
    },
    "andelf_to_patched": compare(andelf,patched),
}
dest = ROOT/"docs/svd-comparison.json"
result = json.dumps(report,indent=2)+"\n"
if "--write" in __import__("sys").argv:
    dest.write_text(result)
else:
    assert dest.read_text() == result, "comparison report stale; run with --write"

# Check important corrections independently of Rust's generated accessor names.
tree = ET.parse(ROOT/"svd/patched.svd").getroot()
ps = {p.findtext("name"): p for p in tree.findall("./peripherals/peripheral")}
for p, addr in {"glb":0x40000000,"aon":0x4000f000,"dma":0x4000c000,"kys":0x4000a900,
                "uart0":0x4000a000,"uart1":0x4000a100,"qdec1":0x4000a840,"qdec2":0x4000a880}.items():
    assert int(ps[p].findtext("baseAddress"),0) == addr
for p, n in [("uart0",45),("uart1",46),("i2c",48),("glb",60)]:
    assert int(ps[p].findtext("./interrupt/value")) == n
fields = ps["glb"].find("./registers/register[name='cgen_cfg1']/fields")
for name, bit in [("uart0",16),("uart1",17),("i2c",19),("timer",21)]:
    assert int(fields.find(f"./field[name='{name}']/bitOffset").text) == bit
for name, offset, access in [
    ("GPIO_INT_MASK1", 0x1a0, "read-write"),
    ("GPIO_INT_STAT1", 0x1a8, "read-only"),
    ("GPIO_INT_CLR1", 0x1b0, "read-write"),
    ("GPIO_INT2_MASK1", 0x1d0, "read-write"),
    ("GPIO_INT2_STAT1", 0x1d4, "read-only"),
    ("GPIO_INT2_CLR1", 0x1d8, "read-write"),
]:
    register = ps["glb"].find(f"./registers/register[name='{name}']")
    assert int(register.findtext("addressOffset"), 0) == offset
    assert register.findtext("access", "read-write") == access
print("SVD source comparison and reviewed base/IRQ/gate assertions OK")
