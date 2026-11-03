"""Remove chiptool's Cortex-M-only interrupt glue, preserving register output.

chiptool bcf538a emits this glue unconditionally, including for RISC-V SVDs.
Exact-count assertions deliberately fail on generator changes.
"""
from pathlib import Path
import re
import sys

p = Path(sys.argv[1])
s = p.read_text()
s, n = re.subn(r"unsafe impl cortex_m :: interrupt :: InterruptNumber for Interrupt \{.*?\} \}", "", s)
assert n == 1, "chiptool interrupt trait output changed"
start = s.index('# [cfg (feature = "rt")] mod _vectors')
brace = s.index("{", start)
depth = 1
end = brace + 1
while depth:
    depth += (s[end] == "{") - (s[end] == "}")
    end += 1
s = s[:start] + s[end:]
s, n = re.subn(r'# \[cfg \(feature = "rt"\)\] pub use cortex_m_rt :: interrupt ; # \[cfg \(feature = "rt"\)\] pub use Interrupt as interrupt ;', "", s)
assert n == 1, "chiptool interrupt re-export output changed"
assert "cortex_m" not in s
# Upstream embeds `git` output in generated docs; archive and clone builds differ.
s, n = re.subn(r'# ! \[doc = "Peripheral access API \(generated using chiptool v0\.1\.0.*?"\]',
               '# ! [doc = "Peripheral access API generated using chiptool bcf538a2e7b8584ae874ee9ab72efb1576fc6152"]', s)
assert n == 1, "chiptool version banner changed"
p.write_text("// Generated; tools/generate-pac.sh. Do not edit.\n#![allow(clippy::all)]\n" + s)
