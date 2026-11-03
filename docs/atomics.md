# Атомики, LR/SC и FPU

## Вывод для текущей реализации

**Не использовать Rust target с `A` для обычной прошивки BL706.**
Применяются IMC или IMFC; critical section сохраняет и восстанавливает MIE,
а `portable-atomic/critical-section` даёт программный fallback. Это атомарность
относительно maskable IRQ одного hart, не синхронизация с DMA, NMI или отладчиком.

Наличие `A` в `misa` недостаточно для LLVM CAS. `compare_exchange` обычно требует
LR/SC; RMW может требовать AMO. Установка feature `critical-section` на A-target
сама по себе не заставляет все зависимости отказаться от аппаратных атомиков.
Также недостаточно передать `-C target-feature=-a` приложению поверх готового
`core` от `riscv32imafc`: target cfg, ABI и ранее собранные библиотеки остаются проблемой.

## Доказательства и неопределённость

| Факт | Что он означает |
|---|---|
| Официальный SVD описывает RV32IMAFC | Маркетинговая/генераторная metadata, не гарантия работы каждой A-инструкции на каждом адресе |
| [SiFive E24 21G3.02.00](https://sifive.cdn.prismic.io/sifive/dc408861-94ce-4d82-a704-caddec98609d_e24_core_complex_manual_21G3.pdf), §3.5, printed p26 / PDF p28 | LR/SC отсутствуют и дают illegal instruction; AMO зависят от memory port; System Port AMO дают access exception |
| [Обсуждение на форуме Bouffalo](https://bbs.bouffalolab.com/d/133-atomic-memory-operations) | Для BL702 сообщается AMO fault на RAM 0x42020000 и других 0x22/0x42 aliases, при этом `misa.A` установлен; ответ подтверждает отсутствие atomic access по шине |
| Конкретный BL706 не исследован на железе | Ревизия SiFive и соответствие ему manual 21G3 неизвестны. Это основание для консервативного target, а не измеренный результат BL706 |

Нельзя переносить успех AMO на `mtimecmp` из forum report на RAM и нельзя делать
такую проверку частью обычного приложения: запись comparator меняет работу таймера.
Не следует также переносить на BL706 свойства SiFive U74/S7 или Xuantie E907.

При гипотетической поддержке LR/SC результат SC=0 означает успех, ненулевой результат
означает неуспех. Многократный SC failure не доказывает неисправность. Но бесконечный
CAS retry loop непригоден как первичная диагностика. Успех одной операции не доказывает
reservation invalidation, работу при IRQ/DMA, поддержку aliases или forward progress.

## Диагностический пример

`atomics` запускается отдельно с feature `atomic-probe` и IMC target:

1. Печатает `misa`, `marchid`, `mimpid` и адрес собственной выровненной `PROBE_WORD` в OCRAM.
2. Отключает maskable IRQ; выполняет одну LR/SC пару, без retry loop.
3. Выполняет один AMOADD на той же собственной RAM-переменной.
4. Печатает return code, значение word и trap record после каждой пробы.

Exception handler восстанавливается только при cause 2/5/7 на **одном из трёх
точных адресов probe-инструкций**. Другие исключения останавливаются.
Он перенаправляет `mepc` в failure continuation целой assembly-функции;
не выполняет произвольное `mepc += 4` и не маскирует произвольные сбои.
Инструкция, которая аппаратно зависла без trap, этим механизмом не исправляется;
нужны отладчик и возможность reset.

Тест касается только собственной word на используемом alias OCRAM. ITCM, другие
RAM aliases, DMA и IRQ-инвалидация reservation остаются отдельными исследованиями.
Пока ни одна проба не запускалась на устройстве.

`audit_elf.py` проверяет executable instruction encodings, включая инструкции,
которые objdump без `+a` может показать как unknown. Во всех обычных ELF A-opcodes
должны отсутствовать. В `atomics` разрешены ровно три адреса с известными labels.

## FPU

FPU поддерживается отдельной IMFC-сборкой с ilp32f ABI. Target встроен в закреплённый
nightly, но не имеет готового rust-std; используется `-Zbuild-std=core`.
В startup до первого Rust-вызова устанавливается `mstatus.FS=Dirty` и обнуляется `fcsr`.
Каждый ненестированный IRQ сохраняет f0..f31 и fcsr, восстанавливает их и mstatus
перед `mret`. Frame 224 байта выровнен на 16; stack зарезервирован 8 KiB.
Программного переключения FP-контекста между preemptive tasks нет: executor cooperative.

Пример `fpu` выполняет runtime f32-вычисление и проверяет битовый результат 3.25,
показывает FS/fcsr и использует Embassy Timer. Сам по себе он ещё не доказывает
сохранность всех FPR при IRQ: необходим отдельный register-sentinel тест под отладчиком.
Локальный ELF-аудит проверяет присутствие spills/reloads всех 32 FPR и float ABI,
но аппаратную работу FPU и обработчиков это не подтверждает.
