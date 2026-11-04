# Atomics, LR/SC and FPU

## Production target decision

**Do not use a Rust target with A for normal BL706 firmware.** Use IMC or IMFC.
Critical sections save/restore MIE and `portable-atomic/critical-section` provides
a software fallback. This is atomic relative to maskable IRQs on one hart, not
synchronization with DMA, NMI or a debugger.

`misa.A` alone is insufficient for LLVM CAS. `compare_exchange` commonly requires
LR/SC; RMW may require AMO. Enabling a `critical-section` feature on an A target does
not force every dependency to avoid hardware atomics. Passing `-C target-feature=-a`
to an application over a prebuilt `riscv32imafc` core is also insufficient: target
cfg, ABI and previously built libraries remain relevant.

## Evidence and uncertainty

| Evidence | Interpretation |
|---|---|
| Official SVD says RV32IMAFC | Generator/marketing metadata, not a guarantee of each A instruction at every address |
| [SiFive E24 21G3.02.00](https://sifive.cdn.prismic.io/sifive/dc408861-94ce-4d82-a704-caddec98609d_e24_core_complex_manual_21G3.pdf), §3.5, printed p26 / PDF p28 | LR/SC are absent and raise illegal instruction; AMO support depends on the memory port; System Port AMOs raise access exceptions |
| [Bouffalo forum discussion](https://bbs.bouffalolab.com/d/133-atomic-memory-operations) | Reports BL702 AMO faults on RAM 0x42020000 and other 0x22/0x42 aliases despite `misa.A`; the reply confirms unsupported bus atomic accesses |
| No BL706 hardware investigation | Its SiFive revision and applicability of manual 21G3 are unknown. The target choice is conservative, not a measured BL706 result |

The forum's successful AMO on `mtimecmp` cannot be generalized to RAM and must not
become a normal application test: writing the comparator changes timer operation.
Do not transfer properties of SiFive U74/S7 or Xuantie E907 to BL706.

If LR/SC were supported, SC=0 would mean success and nonzero failure. Repeated SC
failure alone would not prove a fault. An infinite CAS retry loop is unsuitable as
an initial diagnostic. One success establishes neither reservation invalidation,
IRQ/DMA behavior, alias support nor forward progress.

## Isolated diagnostic

Build `atomics` separately with `atomic-probe` and the IMC target. It:

1. Prints `misa`, `marchid`, `mimpid` and its own aligned OCRAM `PROBE_WORD` address.
2. Disables maskable IRQs and executes one LR/SC pair, with no retry loop.
3. Executes one AMOADD on the same owned RAM word.
4. Prints return codes, word contents and trap records after the probes.

The exception handler recovers only causes 2/5/7 at **three exact probe instruction
addresses**. All other exceptions stop. It redirects `mepc` to the whole assembly
function's failure continuation; it does not blindly add four or suppress unrelated
faults. Hardware hanging without a trap still requires a debugger/reset.

The probes cover only the owned word and chosen OCRAM alias. ITCM, other RAM aliases,
DMA and IRQ reservation invalidation are separate investigations. No probe has run
on a device.

`audit_elf.py` checks executable instruction encodings, including A instructions
that objdump without `+a` may show as unknown. Normal ELF files must contain none.
Only the three labelled instruction addresses are allowed in `atomics`.

## FPU

The separate IMFC build uses ilp32f ABI. The pinned nightly has the built-in target
but no prebuilt rust-std, so `-Zbuild-std=core` is used. Startup sets
`mstatus.FS=Dirty` and clears `fcsr` before the first Rust call. Each nonnested IRQ
saves/restores f0..f31, fcsr and mstatus before `mret`. The 224-byte frame is
16-byte aligned; the stack reservation is 8 KiB. There is no preemptive task FP
context switch: the executor is cooperative.

`fpu` computes a runtime f32 result, checks the bits for 3.25, reports FS/fcsr and
uses an Embassy Timer. It does not prove preservation of every FPR across IRQs;
a debugger-driven register-sentinel test is still required. ELF checks verify all
32 FPR spills/reloads and the float ABI, not physical FPU/handler operation.
