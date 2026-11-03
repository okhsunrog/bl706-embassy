# Источники и происхождение

Проверено 2026-11-04. Ссылки на исходники закреплены на commit, а не на плавающий `master`.

| Источник | Версия / commit | Использование |
|---|---|---|
| [Официальный Bouffalo SDK, SVD](https://github.com/bouffalolab/bouffalo_sdk/blob/50d130075c276fecb9bb3ed286516dcf113531cd/drivers/bl702_driver/regs/soc702_reg.svd) | `50d130075c276fecb9bb3ed286516dcf113531cd` | Неизменный вход PAC |
| [andelf/bl702-pac](https://github.com/andelf/bl702-pac/tree/8ed991ce519e1d7aa37b09584d18aad34285d7de/svd) | `8ed991ce519e1d7aa37b09584d18aad34285d7de` | Сравнение SVD, YAML и результата svdtools |
| [Bouffalo reference manual](https://github.com/bouffalolab/bl_docs/blob/665e7daa84b498a767e656f458f26aca3be4fd45/BL702_RM/en/BL702_BL704_BL706_RM_en_1.2.pdf) | RM 1.2, `665e7daa84b498a767e656f458f26aca3be4fd45` | Memory map, GPIO, UART, I²C |
| [Исходники RM](https://github.com/bouffalolab/bl_docs/tree/665e7daa84b498a767e656f458f26aca3be4fd45/BL702_RM/en/RST) | тот же commit | Поиск и сверка таблиц, включая поля access |
| [Официальный современный SDK](https://github.com/bouffalolab/bouffalo_sdk/tree/237ed937856bf435f4f3642ea4728294723e1d87/drivers/soc/bl702) | `237ed937856bf435f4f3642ea4728294723e1d87` | `std/src/bl702_glb.c`, `bl702_hbn.c`, `startup/start.S`, register headers |
| [SDK LHAL](https://github.com/bouffalolab/bouffalo_sdk/tree/237ed937856bf435f4f3642ea4728294723e1d87/drivers/lhal) | тот же commit | `bflb_i2c.c`, `include/arch/risc-v/e24/clic.h` |
| [rustsbi/bouffalo-hal](https://github.com/rustsbi/bouffalo-hal/tree/588beeda8e07d7876a1b4b98dcf615e427baf4ec) | `588beeda8e07d7876a1b4b98dcf615e427baf4ec` | v1 GPIO layout, UART routing, startup/linker и границы готовности BL702 |
| [IronOS Pinecil V2 BSP](https://github.com/Ralim/IronOS/tree/c7413e0d2adfd43156510d4ccbd109cf352ca335/source/Core/BSP/Pinecilv2) | `c7413e0d2adfd43156510d4ccbd109cf352ca335` | Разводка, рабочая последовательность I²C, локальная копия старого SDK |
| [Схема PINE64](https://files.pine64.org/doc/Pinecil/Pinecil_schematic_v2.0_20220608.pdf) | production v2.0, 2022-06-08 | U15 GPIO10/11, GPIO21, GPIO22, U1 FUSB302, подтяжки |
| [SiFive E24 manual](https://sifive.cdn.prismic.io/sifive/dc408861-94ce-4d82-a704-caddec98609d_e24_core_complex_manual_21G3.pdf) | 21G3.02.00 | §3.5, §3.6, §7.7, §8.6–8.7; **не установлена ревизия ядра BL706** |
| [Bouffalo atomic discussion](https://bbs.bouffalolab.com/d/133-atomic-memory-operations) | 2022-04 | Сообщение о trap AMO на RAM BL702 и ответ о неподдерживаемом доступе по шине |
| [FUSB302B datasheet](https://www.onsemi.com/pdf/datasheet/fusb302b-d.pdf) | tables 15–16 | 7-bit 0x22, DEVICE_ID 0x01; ID зависит от варианта |
| [chiptool](https://github.com/embassy-rs/chiptool/tree/bcf538a2e7b8584ae874ee9ab72efb1576fc6152) | `bcf538a2e7b8584ae874ee9ab72efb1576fc6152` | Генератор, lockfile сохранён в tools |
| [Embassy](https://github.com/embassy-rs/embassy/tree/4c0da6097c5b3ce078473016d7c4654f90693afa) | `4c0da6097c5b3ce078473016d7c4654f90693afa` | Сравнение async I²C и GPIO: RP GPIO/InputFuture, STM32 EXTI one-shot waits, mask/wake/cancel; [GPIO](gpio.md) |

Текущий SDK больше не содержит этого SVD. Исторический файл загружен именно из
официального namespace; его SHA-256 совпадает с копией в IronOS. Версии master_legacy
и v2.3.37-history-base тоже проверялись при поиске, но не используются как вход генерации.

## SHA-256 сохранённых входов

```text
soc702_reg.svd       6e623b60330411b91ac1161cb7414268f622dc282b79705a05b55a2fe44d936b
andelf.svd          3e6fab90941c283413bb028159bb765cf0bbd97b2168ea73df7fb87802380fd1
andelf.svd.patched  2c37d81c66c08ef78cb2554b977623bff91462b078b8d9cf3fe471b273ccea60
andelf.yaml         b5af4e64c99ae08952900e8ce0584e7f338552e7a76ed853ae4e0000e8e04a68
BL70x RM 1.2        d7359b6a833b35e3696ded083739a5faf92502bf9acd3e3621d61fd52bb6d3c2
E24 21G3.02.00      3713b14f7f99a12bb705a2934419e162c34b535f3717461e9147ea809ea08a99
Pinecil schematic   1106f3e8f64b6c69be62667ae635b5151d93fe46f3d1d856ee0c50ff29142e6a
```

PDF и исследовательские checkout оставлены под
`/home/okhsunrog/tmp_zfs/codex_tmp/bl706/references/`, не включены в исходники проекта.
Для PDF использован `mistral_ocr` с выбором страниц и `--tables html`;
схема дополнительно отрисована и просмотрена, поскольку OCR плохо извлекает её цепи.

## Лицензирование

Новый код: MIT OR Apache-2.0. SVD Bouffalo поставляется вместе с Apache-2.0 SDK;
копия его LICENSE сохранена в `svd/vendor/LICENSE-BOUFFALO`.
Файлы andelf происходят из проекта MIT OR Apache-2.0. Сторонние copyright/лицензии
сохраняют силу. Материалы SiFive и схема PINE64 не перелицензируются и не включены.

Из rustsbi использованы подходы к layout v1 GPIO, маршрутизации UART и владению pads.
Его BL702 startup помечен upstream как непроверенный, конфигурация clocks содержит TODO,
а текущий blocking I²C принимает `glb::v2` и фиксированную 38 MHz базу.
Эти реализации не подключены как зависимость: здесь v1 GLB, RC32M, собственный CLIC
runtime и async I²C с таймаутом/отменой. Из исходников rustsbi код дословно не копировался.
