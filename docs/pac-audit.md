# PAC comparison and audit boundaries

Generation: official `soc702_reg.svd` → `patch_svd.py` → chiptool → `finish_pac.py`
→ rustfmt. All inputs are preserved; hashes and pinned links are in [sources](sources.md).

## andelf changes

Machine-readable results: [svd-comparison.json](svd-comparison.json).
`tools/check_svd.py` compares XML by name, normalizes numbers and excludes empty XML
nodes from semantic comparison.

Compared with the official input, andelf's source SVD:

- Adds an RF block with 141 registers at `0x40001000`.
- Removes the incomplete CPU entry, changes name/vendor/description and adds a version.
- Sets the empty AON base to `0x4000F000`.
- Otherwise retains the map, including incorrect DMA/KYS bases.

The YAML replaces two 16-bit `cgen_cfg1` fields with 32 named gates and changes block
descriptions. svdtools also reformats numbers/empty nodes. GPIO arrays are commented
out, so they are not applied corrections. AON/HBN description patches do not appear
in the result because source names are uppercase while YAML uses lowercase.
Both YAML and actual `.svd.patched` output are compared.

## Accepted corrections

| Correction | Basis | Check |
|---|---|---|
| BL706 name/version, remove incomplete CPU | chiptool/parser rejects CPU without revision; Rust target supplies ISA | Parsing/generation |
| AON `0x4000F000` | SDK AON_BASE and internal offsets | Matches andelf/SDK |
| DMA `0x40007000` → `0x4000C000` | RM memory map, SDK DMA_BASE | XML and PAC const |
| KYS `0x4000AB00` → `0x4000A900` | RM memory map, SDK KYS_BASE | XML and PAC const |
| UART1 `0x4000A100`, QDEC1/2 `0x4000A840/880` | RM/SDK | Reuse instance layout |
| UART0/1/I²C/GPIO IRQ 45/46/48/60 | SDK IRQ_NUM_BASE=16, RM sources29/30/32/44 | XML and HAL CLIC map |
| cgen_cfg1 UART0=16, UART1=17, SPI=18, I²C=19, PWM=20, TIMER=21, IR=22 | SDK clock/ungate code | Named setters, XML assertions |
| GPIO input/status RO, both clear RW; I²C FIFO RO/WO; UART status/RX RO, TX/clear WO | RM access tables, SDK headers | Generated R/RW/W types |
| ADC FIFO/results RO; TIMER count/status RO, interrupt clear WO; ADC/PWM/TIMER0/1 IRQ25/49/52/53 | RM ADC/TIMER, SDK IRQ+16 | Access/base/IRQ assertions |
| Watchdog WVR RO, WCR/WFAR/WSAR WO | RM TIMER register access | XML assertions and generated PAC |

Other gates remain grouped. andelf names KYS bit25 while modern SDK uses bit27;
bits24..26 also involve QDEC/I²S. They are not renamed or used by this HAL. The
`MAX` bit31 and speculative `CCI/L1C (...efuse?)` descriptions are not imported as
verified facts. RF is not imported: it is a substantial addition without a
manufacturer specification verified here. The official SVD remains the generation input.

## Reviewed HAL paths

- GLB/HBN clock roots, dividers, UART/I²C/mtime muxes, AON XTAL and DLL sequencing,
  RC32K power/selection, IROM2T and ITCM BCLK latch paths compared with SDK.
- Watchdog protected accesses, reset mode and shared TIMER fields; HBN reset flags
  and GLB system+CPU reset. See [system.md](system.md).
- Packed GPIO configuration: two pads per word, IE/SMT/pull/function, data/OE
  offsets `0x180/188/190` and GPIO23..28 SF/PSRAM pad remapping.
- Both GPIO MASK/STAT/CLR/MODE banks, IRQ60, async modes4..7 and packed three-bit
  fields. GPIO_INT_CLR1's earlier WO annotation was corrected to RW per RM and SDK
  1→0 pulse; second STAT is RO. One-shot masking, clear and cancellation are
  documented in [gpio.md](gpio.md).
- UART0 divider, free-run, 8N1, FIFO counts, data and signal mux.
- ADC GPIP/AON clock/divider, PGA/chop/resolution/scan, FIFO tags/W1C and read-only
  gain trim at EF_DATA_0 shadow+0x78, parity/sign conversion. GPIP bit2 and EF_CTRL
  bit7 match SDK ungate code. No eFuse programming.
- Five PWM layouts at stride0x20, BCLK/divider/period/threshold/SW force/STOP and
  shared interrupt clear pulse with channel-local enables. TIMER shared fields,
  three compares/channel, preload and IRQ clear. TCVWR remains RW despite RM RO:
  SDK writes1 then reads three times for capture. SDK compare-minus-two is not
  silently applied. See [analog.md](analog.md).
- I²C packet/sub-address lengths N−1, LSB-first sub-address, repeated START, two-word
  FIFO and mixed END/NAK/ARB/FIFO error/mask/enable/W1C register. ISR buffer service
  avoids echoing W1C while changing masks. See [i2c.md](i2c.md).
- SF_CTRL SAHB owner/clock/command/address/length/TRIG/BUSY and 256-byte SRAM path,
  compared with old SDK SFlash/SF_CTRL code. Only the narrow RAM-only SPI1-1-1 path
  is implemented; no complete flash configuration/XIP protocol. See [flash.md](flash.md).
- Core-local CLIC/mtime are absent from the SVD. Addresses come from SDK e24/clic.h
  and were compared with the SiFive manual. This is a small handwritten HAL layer.

## What is not established

This is not a complete SoC PAC audit. DMA channels, crypto, eFuse writes, USB, RF,
camera/MJPEG and the remaining flash/XIP configuration were not fully checked.
An accessor does not establish availability on a particular package/BL706 variant.
In particular, source SVD entries for MJPEG and ef_data_1 are not verified Pinecil features.

Many source registers inherit RW and global reset=0 despite mixed access or nonzero
silicon defaults. Only listed paths have corrected access. chiptool fieldset Default
is zero, not a silicon-reset guarantee. Do not use generic RMW on mixed status/control;
I²C unmask writes explicitly keep W1C zero.

chiptool emits Cortex-M InterruptNumber/vector glue even for RISC-V. The finishing
script removes precisely that glue with match-count assertions, retaining generated
IRQ enums/MMIO. The CLIC runtime is separate. A changed generator format requires
an explicit script update, rather than silently skipped regex matches.
