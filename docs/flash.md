# Low-level NOR flash

## XIP region handle

`flash::XipFlash::new_mx25l8006` adds a separate ROM-backed path for the
MX25L8006 (`c2 20 14`) with the validated dual-output, no-wrap boot profile.
Its offsets are relative to an explicitly reserved erase-aligned range.
The image offset must be `0x1000`. The constructor checks the linked RAM
trampoline address, ROM API pointers and the actual JEDEC ID.

The trampoline and fail-closed reset are copied into executable RAM by the
linker scripts. It masks MIE, calls the SDK ROM XIP helpers (table entries
78/79/81), flushes the cache (121), fences instructions and restores the previous
MIE state. Configuration and write buffers reside in RAM. ROM failures reset
from RAM rather than returning to an uncertain XIP mapping. Writes and erases
are verified; an already erased sector does not incur another physical erase.

This is not a background erase engine: interrupts are masked for each physical
page program or sector erase. Callers must coordinate heating/radio/PD and must
reserve the region in their image/linker layout. Async mutations yield only
between pages/sectors; reads also yield to bound repeated record scanning.
Cancellation cannot interrupt a physical write already in progress.
Physical sector erase is disabled by default (`Error::Busy`); callers explicitly
enable it only in a suitable maintenance window. Blank-sector erase requests
are no-ops. Metrics expose program/erase counts and their maximum blocking time.
The SDK core-clock shadow (`HBN_RSV2`) is kept current for ROM delay helpers.
The XIP backend has been exercised on Pinecil V2 in
[pinecil-rs](https://github.com/okhsunrog/pinecil-rs#persistent-settings):
settings programming/readback, region erase/GC and reset recovery, with heating
OFF. Long erases require a radio/PD maintenance window; read scans yield to
avoid starving the application watchdog.

The original RAM-only driver below retains its existing contract.

The driver implements `embedded_storage::nor_flash::{ReadNorFlash,NorFlash}` and
their `embedded-storage-async` counterparts. It supplies read/write/erase, without
a settings format, wear leveling or boot image. Applications may use
`sequential-storage` above it; that crate is not a HAL dependency.

**The RAM-only backend below has not been hardware-tested.** Its host tests and
compilation do not establish controller operation, chip compatibility or data
integrity. The ROM-backed XIP implementation has separate qualification above.

## Initial implementation contract

This driver requires **RAM-only** firmware. The unsafe
`Probe::new(p.flash, p.clocks)` constructor requires all code, constants, stack,
buffers and ISRs to remain in RAM/ROM, with no other SF_CTRL/XIP clients, DMA or
debugger accesses. Board/ROM must have configured the correct flash pads and IO
timing; the chip must be awake in SPI 1-1-1 mode, outside QPI and continuous-read.
These conditions have not been established on Pinecil hardware. The unsafe block
in the example does not prove them: verify the contract before running the probe.
Flash pads and configuration must remain exclusively reserved for the handle's lifetime.

SF_CTRL uses SAHB ownership and XCLK32M / 4 = 8 MHz, independent of CPU DLL rate.
Data passes through the 256-byte SRAM window at `0x4000b700`, never caller pointers
or DMA. Commands use SAHB registers +0x08/+0x0c/+0x10, SPI 1-1-1, a one-byte opcode,
an optional 24-bit address, up to 256 data bytes and no dummy clocks. TRIG is lowered
before buffer/command preparation and raised afterward. Synchronous controller
BUSY waits have a nominal 1 ms deadline and a hard poll budget.

There is no QPI exit, continuous-read reset, flash reset/power-up, pin remap,
QE/protection write, SFDP discovery, four-byte addressing, suspend/resume or complete
XIP/cache save/restore. Normal Drop restores saved controller/clock fields; this
is **not** an XIP execution-resumption protocol. Pending/poisoned operations prevent
owner restoration. Stay RAM-only until reset or complete external reinitialization.

## Probe and writable region

`Probe` only reads JEDEC ID/status and data. Density bytes 0x13..0x18 are interpreted
as log2(capacity in bytes), giving metadata for 512 KiB..16 MiB. IDs of 00/FF and
other densities are rejected; this is not a universal JEDEC/SFDP parser.

`into_region(expected_jedec, physical_range)` converts a Probe into `Flash`.
The application must reserve a range containing no firmware or unrelated data.
The full expected ID, capacity, start<end and 4 KiB alignment are checked.
Prefixes EF40 (Winbond W25Q), C840 (GigaDevice GD25Q), and exact ID C22014
(Macronix MX25L8006) allow writes: their
common command path was compared with SDK flash configurations. This does not
qualify every chip in those families. Pinecil's C22014 ID was read through the
separate ROM-backed XIP path; that does not qualify the RAM-only SPI transport.

```rust,ignore
// First establish the RAM/SPI/board contract and verify the actual chip.
let probe = unsafe { Probe::new(p.flash, p.clocks)? };
let chip = probe.chip();
// The application supplies expected_id and reserved_range after checking the layout.
let mut flash = probe.into_region(expected_id, reserved_range)?;
// All NorFlash offsets are relative to reserved_range.start.
```

Traits report READ_SIZE=1, WRITE_SIZE=1, ERASE_SIZE=4096 and the region's capacity.
`MultiwriteNorFlash` is not implemented; do not infer unlimited partial programs
of a physical word/page. Entire request bounds are checked before side effects;
empty requests are no-ops. Erase uses an exclusive end and rejects reversed,
unaligned and overflowing ranges.

Read uses opcode03 in chunks <=256 bytes. Page program uses opcode02 without
crossing a 256-byte boundary; sector erase uses opcode20 for 4096 bytes.
Each mutation requires WREN06 and SR1.WEL verification; SR1 is read with05.
Block protection is never disabled. A 0→1 program attempt returns `NeedsErase`.
Program data and the entire erased sector are read back for verification.
No chip-erase, OTP or eFuse operations are provided.

## Async, timeout and cancellation

Both frontends use the same production engine. Blocking WIP waits spin with IRQs
enabled. Async WIP waits yield via a 1 ms Embassy Timer; read chunks, pages and
erase verification chunks also yield cooperatively. Each controller transaction
remains synchronous and bounded, rather than a DMA or fully nonblocking transfer.

A pending deadline is stored in `Flash` before each destructive command.
Cancelling or forgetting a future preserves that state and leaves no caller memory
address in hardware. The next request checks the old WIP before proceeding.
A cancelled multi-page request may be partially completed and is not automatically
resumed. Starting another operation does not imply the cancelled one was successful
or atomic.

Busy limits are 10 ms for program and 2 s for sector erase, based on conservative
SDK profile limits; actual chip/voltage/temperature limits still need verification.
They require working `mtime`. Controller failure or WIP persisting beyond the
deadline poisons the handle; subsequent IO returns `Poisoned`. Reset and reestablish
the flash contract; there is no automatic chip reset/recovery. Drop with a pending
or poisoned operation does not interrupt program/erase or switch ownership to XIP.
Physical writes cannot be rolled back by cancelling a future.

## Example and validation

`flash_probe` only reads ID and the first 16 bytes, then logs metadata. Runtime code
does not create a writable region. Blocking/async read/write/erase paths are also
instantiated in compile-only wrappers retained by `black_box` function pointers.
Main/tasks never call those wrappers. ELF checks require their presence in RAM and
check IMC A/FP exclusion. This checks compilation/placement, not execution of writes,
even in an emulator.

Eight host tests run the production NOR engine with a memory-backed transport:
bounds/overflow/alignment, expected-ID gating, command encoding, page splitting,
WREN/WEL/program/erase/readback, 0→1 rejection, protection, persistent pending state,
BUSY/timeout/poisoning. The model does not emulate SF_CTRL, SRAM timing, electrical
SPI, real WIP, cache coherence, power loss or endurance. Hardware scenarios and
write prerequisites are in [hardware.md](hardware.md).

Sources: official SDK [SF control](https://github.com/bouffalolab/bl_iot_sdk/blob/edcf613f073d3ef7d2f290d7c285b69be35da75b/components/platform/soc/bl702/bl702_std/BSP_Driver/std_drv/src/bl702_sf_ctrl.c),
[SFlash](https://github.com/bouffalolab/bl_iot_sdk/blob/edcf613f073d3ef7d2f290d7c285b69be35da75b/components/platform/soc/bl702/bl702_std/BSP_Driver/std_drv/src/bl702_sflash.c),
[flash configs](https://github.com/bouffalolab/bl_iot_sdk/blob/edcf613f073d3ef7d2f290d7c285b69be35da75b/components/platform/soc/bl702/bl702_std/BSP_Driver/std_drv/src/bl702_sf_cfg.c),
headers and corresponding IronOS paths. SDK boot/XIP wrappers were inspected to
identify boundaries; their functionality is not claimed as implemented here.
