# BL706 PAC и Embassy bring-up

Начальная поддержка Bouffalo Lab BL702/704/706, ориентированная на **Pinecil V2 / BL706**.
Железо недоступно: ни загрузка, ни UART, ни таймеры, ни I²C пока не проверены на плате.
Это исследовательская основа, не замена IronOS и не готовая прошивка паяльника.

В workspace:

- `bl706-pac`: полный набор блоков из официального SVD, генерация **chiptool** с проверяемыми исправлениями. Проверена только часть регистров; границы аудита описаны отдельно.
- `bl706-hal`: RAM startup, nominal RC32M, GPIO, UART0 TX, CLIC, Embassy executor и time driver, IRQ-driven async I²C для регистровых устройств.
- `examples/pinecil-v2`: heartbeat, чтение FUSB302, GPIO IRQ, проверка задержки executor, FPU и отдельная ограниченная диагностика атомиков.

## Сборка

```sh
nix develop
cargo firmware --locked
cargo firmware-fpu --locked
```

`flake.lock`, `Cargo.lock`, `rust-toolchain.toml` и commit chiptool закреплены.
Nix предоставляет компилятор, `rust-src`, chiptool, uv, LLVM и OpenOCD.
Первая сборка скачивает зависимости Cargo; следующие можно запускать с `--offline`.
Для IMC-примеров есть отдельная изолированная Nix-сборка с vendored Cargo dependencies:

```sh
nix build .#firmware
# result/bin/{heartbeat,fusb302,i2c_latency,gpio_irq}.elf
nix flake check
```

`checks` собирают IMC ELF и заново генерируют PAC с побайтовым сравнением.
IMFC/build-std и диагностический binary проверяются через `tools/check.sh` в devShell.

| Конфигурация | Target | ABI | FP-контекст IRQ |
|---|---|---|---|
| IMC | `riscv32imc-unknown-none-elf` | ilp32 | FP не используется |
| IMFC, `fpu` | `riscv32imfc-unknown-none-elf` | ilp32f | f0..f31, fcsr, mstatus |

IMFC является встроенным tier-3 target Rust. Alias `firmware-fpu` пересобирает `core`
через `-Zbuild-std=core`; custom JSON не нужен. **Обе конфигурации исключают `A`.**
Сборка HAL для A-target или IMFC без `fpu` намеренно отклоняется.

ELF находятся в `target/<target>/release/{heartbeat,fusb302,i2c_latency,gpio_irq,fpu}`.
Все они предназначены для загрузки **в RAM через JTAG/GDB**. Они не содержат flash/boot header;
передавать их или результат обычного objcopy в `blisp flash` нельзя.

```sh
# Изолированный бинарник с тремя намеренными A-инструкциями:
cargo build --locked --release -p pinecil-v2-bringup --bin atomics \
  --features atomic-probe --target riscv32imc-unknown-none-elf

# PAC, обе конфигурации, clippy, host-тесты, структура ELF и машинные инструкции:
bash tools/check.sh
```

Скрипт оставляет диагностические файлы в
`/home/okhsunrog/tmp_zfs/codex_tmp/bl706/checks/`;
на другой машине задайте `BL706_DIAGNOSTICS`. Обычные сборки используют `target/`.

## PAC

```sh
bash tools/generate-pac.sh
uv run --no-project tools/check_svd.py
```

Исходный SVD и обе версии andelf сохранены неизменными в `svd/vendor/`.
`patch_svd.py` применяет исправления до chiptool; `finish_pac.py` удаляет его Cortex-M glue
и нормализует баннер генератора. Редактировать сгенерированный `lib.rs` вручную не нужно.

Прямой PAC намеренно не обеспечивает владение периферией. Его безопасные MMIO-методы
могут нарушить инварианты HAL. Не смешивайте прямую запись в PAC с активным драйвером.

## Ограничения

- Только одно ядро, M-mode, один thread executor с активным ожиданием. Вложенные IRQ и энергосбережение не поддерживаются.
- RC32M имеет номинальную, а не измеренную частоту. PLL 144 MHz, XIP, загрузка из flash, RF, USB и DMA не реализованы.
- UART0: TX на GPIO22, 115200 8N1 в примерах. RX GPIO23 не реализован.
- GPIO: входы/выходы, IRQ-driven `embedded_hal_async::digital::Wait` (low/high/rising/falling/any-edge), отмена ожидания. Подробности: [GPIO](docs/gpio.md). FUSB302 INT_N на GPIO16; debounce и deep-sleep wakeup не реализованы.
- I²C: один пакет 1..256 байт; `write_read` с 1..4 байтами sub-address использует hardware repeated START. Другие формы `transaction` и пустые пакеты возвращают `Unsupported` до начала обмена. Автоматического bit-bang bus recovery нет.
- FIFO обслуживается из ISR; задача только ждёт результат. Используется внутренний 256-байтный packet buffer, чтобы ISR не хранил указатели на память вызывающего кода. Подробности и отмена: [I²C](docs/i2c.md).
- Read-only probe FUSB302 не реализует USB-PD policy engine и не запрашивает повышенное напряжение.
- Time queue рассчитана на восемь различных ожидающих waker. Это начальное ограничение конфигурации, не общий предел Embassy.

Подробности: [источники](docs/sources.md), [аудит PAC](docs/pac-audit.md),
[атомики и FPU](docs/atomics.md), [аппаратные допущения и проверки](docs/hardware.md),
[результаты локальной проверки](docs/validation.md).
