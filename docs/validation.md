# Локальная проверка

2026-11-04, host x86_64-linux/NixOS. **Плата не подключалась, аппаратных результатов нет.**

## Пройдено

| Проверка | Результат |
|---|---|
| `nix build .#chiptool --no-link` | Pinned chiptool и git SVD parser собраны с lockfile/fixed-output hashes |
| `nix build .#firmware .#checks.x86_64-linux.pac --no-link` | Успех: IMC heartbeat/fusb302 собраны в изолированном builder; PAC регенерируется побайтово одинаково |
| `nix flake check` | Оценка packages/checks/devShell и обе sandbox checks на x86_64-linux успешны |
| `nix develop -c bash tools/check.sh` | Весь набор ниже прошёл с exit 0 |
| Сравнение SVD | Неизменные source hashes, semantic comparison и assertions исправленных bases/IRQ/gates |
| `cargo fmt --all --check` | Успех |
| Host PAC build/test/doc-test | Успех компиляции; собственных unit tests у сгенерированного PAC нет |
| Host I²C FIFO model | 3 теста: sub-address byte order, TX backpressure/RX streaming для всех длин 1..256, отсутствие I/O/progress при full/empty FIFO |
| IMC release | heartbeat, fusb302, atomics link успешно |
| IMFC release + build-std=core | heartbeat, fusb302, fpu link успешно |
| Clippy `-D warnings` | IMC с atomic-probe и IMFC с fpu проходят; generated PAC отдельно допускает стандартные generator lints |
| ELF32 audit | RAM bounds, VMA=LMA, entry, stack 8 KiB, vector table 80 entries, mtvec/mtvt alignment 64, ABI |
| Instruction encoding audit | В обычных 5 ELF нет A-opcodes; в atomics ровно 3 на точных probe labels; в IMC нет FP; в IMFC есть save/restore всех 32 FPR |

Регенерация с chiptool из git checkout и с Nix archive build даёт одинаковый PAC:

```text
6d3081cb701ee411e0b671c461746ffe2a0a4386acac24de3a32d11cc0d58550
```

Для этого нормализован только banner генератора, содержащий необязательный git commit/date.
Обычная прошивка, собранная в checkout и Nix sandbox, может отличаться debug paths;
битовая идентичность этих ELF между разными путями сборки не проверялась.

Полные логи текущего выполнения находятся вне репозитория:

```text
/home/okhsunrog/tmp_zfs/codex_tmp/bl706/final-check.log
/home/okhsunrog/tmp_zfs/codex_tmp/bl706/nix-hermetic.log
/home/okhsunrog/tmp_zfs/codex_tmp/bl706/nix-final-flake-check.log
```

## Практические границы

Host-модель проверяет используемые драйвером FIFO pumps, но не моделирует BL706,
CLIC, W1C side effects, время появления END/NAK, IRQ latency или физический STOP.
Проверка наличия FP spills/reloads в ELF не заменяет runtime register-sentinel test.
Nix sandbox success не подтверждает работоспособность startup/clock tree.

Ожидают железа: RAM reset/load contract, такты/точность UART, возврат из CLIC IRQ,
mtime deadlines/rollover, GPIO pad mapping, I²C repeated START/FIFO/errors/cancel,
FUSB302 ID и INT_N, FPU/context, а также traps/результаты LR/SC и AMO на конкретной
ревизии BL706. Подробный порядок — [hardware.md](hardware.md).

AArch64 Nix environment объявлен, но сборкой здесь не проверялся. Отдельного QEMU
или register-level simulator для BL706 не запускалось; эмуляция другого RISC-V SoC
не считалась бы подтверждением BL706.
