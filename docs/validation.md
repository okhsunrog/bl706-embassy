# Локальная проверка

2026-11-04, host x86_64-linux/NixOS. **Плата не подключалась, аппаратных результатов нет.**

## Пройдено

| Проверка | Результат |
|---|---|
| `nix build .#chiptool --no-link` | Pinned chiptool и git SVD parser собраны с lockfile/fixed-output hashes |
| `nix build .#firmware .#checks.x86_64-linux.pac --no-link` | Первоначальная проверка sandbox IMC firmware и PAC; актуальный набор проверяется через `nix flake check` |
| `nix flake check` | Оценка packages/checks/devShell и обе sandbox checks на x86_64-linux успешны |
| `nix develop -c bash tools/check.sh` | Весь набор ниже прошёл с exit 0 |
| Сравнение SVD | Неизменные source hashes, semantic comparison и assertions исправленных bases/IRQ/gates |
| `cargo fmt --all --check` | Успех |
| Host PAC build/test/doc-test | Успех компиляции; собственных unit tests у сгенерированного PAC нет |
| Host I²C transfer engine | 10 тестов: TX/RX для всех длин 1..256 без polls waiter; bounded FIFO service; END/error races; late final word; cancel/stale IRQ/reuse; независимость TX от caller storage |
| Host GPIO IRQ engine | 7 тестов: mode fields всех 32 пинов, level/edge/any-edge, событие до poll, level/unmask race, несколько пинов, cancel/stale IRQ/rearm |
| Host ADC/PWM/TIMER logic | 10 тестов: ADC engine lengths 1..12 без waiter polls, ошибки до/после drain, tags/extra/cancel/reuse; restart gate; scan packing; resolution; все 4096 gain trims/parity/sign/clamp; PWM limits; TIMER divider/preload и shared field preservation |
| IMC release | heartbeat, fusb302, i2c_latency, gpio_irq, analog, sampling, atomics link успешно |
| IMFC release + build-std=core | heartbeat, fusb302, i2c_latency, gpio_irq, analog, sampling, fpu link успешно |
| Clippy `-D warnings` | IMC с atomic-probe и IMFC с fpu проходят; generated PAC отдельно допускает стандартные generator lints |
| ELF32 audit | RAM bounds, VMA=LMA, entry, stack 8 KiB, vector table 80 entries с handlers 7/25/48/49/52/53/60, mtvec/mtvt alignment 64, ABI |
| Instruction encoding audit | В обычных 13 ELF нет A-opcodes; в atomics ровно 3 на точных probe labels; в IMC нет FP; в IMFC есть save/restore всех 32 FPR |

Регенерация с chiptool из git checkout и с Nix archive build даёт одинаковый PAC:

```text
72d6e7dad00c03acb0d9231636a49d0d753555c6d6b9e833b1bc7bccc5cb7153
```

Для этого нормализован только banner генератора, содержащий необязательный git commit/date.
Обычная прошивка, собранная в checkout и Nix sandbox, может отличаться debug paths;
битовая идентичность этих ELF между разными путями сборки не проверялась.

Полные логи текущего выполнения находятся вне репозитория:

```text
/home/okhsunrog/tmp_zfs/codex_tmp/bl706/analog-check.log
/home/okhsunrog/tmp_zfs/codex_tmp/bl706/analog-nix-check.log
```

## Практические границы

Host-модель проверяет используемый ISR transfer engine, включая stop/mask на
завершении, но не моделирует BL706, CLIC, W1C side effects, реальное время появления
END/NAK, IRQ latency или физический STOP. `i2c_latency` предназначен для аппаратной
проверки прогресса девятибайтного чтения при блокировке executor на 20 ms;
фактически этот сценарий на устройстве ещё не выполнялся. Владение packet memory
и ограничения отмены описаны в [i2c.md](i2c.md).
GPIO-модель исполняет production state engine, но не MMIO/waker/future Drop glue
на RISC-V и не реальные GPIO/CLIC. `gpio_irq` только собран; CLR pulse, async modes,
маршрутизация обоих детекторов, latency и отсутствие IRQ storm требуют платы.
См. [gpio.md](gpio.md).
ADC/PWM/TIMER host-тесты исполняют pure logic и ADC transfer engine, но не MMIO,
реальный FIFO/status/W1C, waker/Drop glue на MCU или timer callbacks. `analog` и
`sampling` только собраны; accuracy/clock/settling, FIFO tags, capture, compare
offset, PWM waveform, IRQ latency и fail-off timing не измерялись. В обоих примерах
PWM unrouted и GPIO21 low; регулятор нагрева не реализован. См. [analog.md](analog.md).
Проверка наличия FP spills/reloads в ELF не заменяет runtime register-sentinel test.
Nix sandbox success не подтверждает работоспособность startup/clock tree.

Ожидают железа: RAM reset/load contract, такты/точность UART, возврат из CLIC IRQ,
mtime deadlines/rollover, GPIO pad mapping/IRQ/levels/edges/cancel, I²C repeated START/FIFO/errors/cancel,
FUSB302 ID и INT_N, FPU/context, а также traps/результаты LR/SC и AMO на конкретной
ревизии BL706; ADC readings/calibration, PWM waveform/IRQ и TIMER preload/capture.
Подробный порядок — [hardware.md](hardware.md).

AArch64 Nix environment объявлен, но сборкой здесь не проверялся. Отдельного QEMU
или register-level simulator для BL706 не запускалось; эмуляция другого RISC-V SoC
не считалась бы подтверждением BL706.
