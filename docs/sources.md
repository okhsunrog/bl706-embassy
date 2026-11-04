# Sources and provenance

Reviewed 2026-11-04. Source links use pinned commits rather than floating branches.

| Source | Version / commit | Use |
|---|---|---|
| [Official Bouffalo SDK SVD](https://github.com/bouffalolab/bouffalo_sdk/blob/50d130075c276fecb9bb3ed286516dcf113531cd/drivers/bl702_driver/regs/soc702_reg.svd) | `50d130075c276fecb9bb3ed286516dcf113531cd` | Immutable PAC input |
| [andelf/bl702-pac](https://github.com/andelf/bl702-pac/tree/8ed991ce519e1d7aa37b09584d18aad34285d7de/svd) | `8ed991ce519e1d7aa37b09584d18aad34285d7de` | SVD/YAML/svdtools-output comparison |
| [Bouffalo reference manual](https://github.com/bouffalolab/bl_docs/blob/665e7daa84b498a767e656f458f26aca3be4fd45/BL702_RM/en/BL702_BL704_BL706_RM_en_1.2.pdf) | RM1.2, `665e7daa84b498a767e656f458f26aca3be4fd45` | Memory map and peripheral registers |
| [RM sources](https://github.com/bouffalolab/bl_docs/tree/665e7daa84b498a767e656f458f26aca3be4fd45/BL702_RM/en/RST) | Same commit | Searchable tables and access annotations |
| [Modern official SDK](https://github.com/bouffalolab/bouffalo_sdk/tree/237ed937856bf435f4f3642ea4728294723e1d87/drivers/soc/bl702) | `237ed937856bf435f4f3642ea4728294723e1d87` | GLB/AON/HBN/clocks, startup and register headers |
| [SDK LHAL](https://github.com/bouffalolab/bouffalo_sdk/tree/237ed937856bf435f4f3642ea4728294723e1d87/drivers/lhal) | Same commit | I²C/GPIO/ADC/PWM/TIMER/watchdog, e24/clic.h |
| [Official bl_iot_sdk BL702](https://github.com/bouffalolab/bl_iot_sdk/tree/edcf613f073d3ef7d2f290d7c285b69be35da75b/components/platform/soc/bl702/bl702_std/BSP_Driver/std_drv) | `edcf613f073d3ef7d2f290d7c285b69be35da75b` | ADC/PWM/TIMER sequences, SF_CTRL/SFlash/config profiles; compared with IronOS/LHAL |
| [rustsbi/bouffalo-hal](https://github.com/rustsbi/bouffalo-hal/tree/588beeda8e07d7876a1b4b98dcf615e427baf4ec) | `588beeda8e07d7876a1b4b98dcf615e427baf4ec` | v1 GPIO layout, UART routing, startup/linker and BL702 readiness limits |
| [IronOS Pinecil V2 BSP](https://github.com/Ralim/IronOS/tree/c7413e0d2adfd43156510d4ccbd109cf352ca335/source/Core/BSP/Pinecilv2) | `c7413e0d2adfd43156510d4ccbd109cf352ca335` | Board mapping, working peripheral sequences, local old SDK copy |
| [PINE64 schematic](https://files.pine64.org/doc/Pinecil/Pinecil_schematic_v2.0_20220608.pdf) | Production v2.0, 2022-06-08 | U15 GPIO10/11/21/22, U1 FUSB302, pull-ups |
| [SiFive E24 manual](https://sifive.cdn.prismic.io/sifive/dc408861-94ce-4d82-a704-caddec98609d_e24_core_complex_manual_21G3.pdf) | 21G3.02.00 | §3.5/3.6/7.7/8.6–8.7; BL706 core revision is unknown |
| [Bouffalo atomic discussion](https://bbs.bouffalolab.com/d/133-atomic-memory-operations) | 2022-04 | BL702 RAM AMO trap report and bus-access reply |
| [FUSB302B datasheet](https://www.onsemi.com/pdf/datasheet/fusb302b-d.pdf) | Tables15–16 | 7-bit address0x22, DEVICE_ID0x01; variant-dependent ID |
| [chiptool](https://github.com/embassy-rs/chiptool/tree/bcf538a2e7b8584ae874ee9ab72efb1576fc6152) | `bcf538a2e7b8584ae874ee9ab72efb1576fc6152` | Generator, lockfile retained in tools |
| [Embassy](https://github.com/embassy-rs/embassy/tree/4c0da6097c5b3ce078473016d7c4654f90693afa) | `4c0da6097c5b3ce078473016d7c4654f90693afa` | Async I²C/GPIO comparison, RP InputFuture, STM32 EXTI mask/wake/cancel; see [GPIO](gpio.md) |

The modern SDK no longer includes this SVD. The historical input was downloaded
from the official namespace; its SHA-256 matches IronOS's copy. master_legacy and
v2.3.37-history-base were also inspected during discovery but are not generation inputs.

## Preserved input SHA-256

```text
soc702_reg.svd       6e623b60330411b91ac1161cb7414268f622dc282b79705a05b55a2fe44d936b
andelf.svd          3e6fab90941c283413bb028159bb765cf0bbd97b2168ea73df7fb87802380fd1
andelf.svd.patched  2c37d81c66c08ef78cb2554b977623bff91462b078b8d9cf3fe471b273ccea60
andelf.yaml         b5af4e64c99ae08952900e8ce0584e7f338552e7a76ed853ae4e0000e8e04a68
BL70x RM 1.2        d7359b6a833b35e3696ded083739a5faf92502bf9acd3e3621d61fd52bb6d3c2
E24 21G3.02.00      3713b14f7f99a12bb705a2934419e162c34b535f3717461e9147ea809ea08a99
Pinecil schematic   1106f3e8f64b6c69be62667ae635b5151d93fe46f3d1d856ee0c50ff29142e6a
```

PDFs and research checkouts remain outside the repository under
`/home/okhsunrog/tmp_zfs/codex_tmp/bl706/references/`. PDF extraction used
`mistral_ocr` with selected pages and `--tables html`; the schematic was also rendered
and inspected visually because OCR poorly captures its connectivity.

## Licensing

New code: MIT OR Apache-2.0. Bouffalo SVD comes from the Apache-2.0 SDK; its license
is preserved in `svd/vendor/LICENSE-BOUFFALO`. andelf inputs come from an
MIT OR Apache-2.0 project. Third-party copyright/licenses remain applicable.
SiFive materials and the PINE64 schematic are neither included nor relicensed.

rustsbi informed v1 GPIO layout, UART routing and pad ownership. Its upstream BL702
startup is marked untested, clock configuration has TODOs, and its current blocking
I²C takes `glb::v2` with a fixed38 MHz base. Those implementations are not dependencies:
this HAL uses v1 GLB, configurable BL702 clocks, a separate CLIC runtime and async
I²C with timeout/cancellation. No rustsbi code was copied verbatim.
