# Допущения и план проверки Pinecil V2

## Контракт первого запуска

- BL706, один hart, machine mode, холодный reset/ROM ISP; не attach к работающей IronOS.
- Отладчик загружает все PT_LOAD segments ELF по их VMA=LMA. `.data` не копируется
  из flash: debugger уже загрузил её. `.bss` обнуляется startup.
- `.init` и clock transition находятся в ITCM `0x22010000..0x22013fff`;
  остальные sections/stack — в нижних 48 KiB OCRAM `0x22020000..0x2202bfff`.
  Верхние 16 KiB OCRAM оставлены под возможное reset/ROM EM allocation.
- ITCM/OCRAM доступны после reset без изменения cache partition. Это допущение
  необходимо проверить для конкретного BL706/ROM и debug reset sequence.
- RC32M включён после reset. Выбирается RC32M root, HCLK/BCLK div=0. Частота номинальная;
  trim не калибруется, XTAL32M/DLL144M не включаются.
- Код изменения bus divider исполняется в ITCM без stack accesses: SDK предупреждает
  о краткой остановке BCLK/OCRAM при latch divider через `0x40000ffc`.
- CLIC использует SDK layout: IP +0x000, IE +0x400, CFG +0x800, global +0xc00.
  `mtvec=handler|3`, `mtvt=vector table`; оба base выровнены на 64 байта.
  Нестирование запрещено, handler не включает MIE.
- Внешние I²C pull-ups существуют на плате; частота/фронты не измерены.

## Разводка

Сверено с U15 производственной [схемы v2.0](https://files.pine64.org/doc/Pinecil/Pinecil_schematic_v2.0_20220608.pdf)
и SDK alternate functions. Сигнальные имена UART1 на схеме обозначают сеть платы;
в примерах к GPIO22 маршрутизируется контроллер **UART0 TX**.

| Сигнал | GPIO | Действие примеров |
|---|---|---|
| Tip_Control | 21 | GPIO output low; heater не включается |
| UART TX | 22 | function7, signal6 → UART0_TXD, 115200 8N1 |
| UART RX | 23 | драйвер RX не реализован |
| I²C SCL | 10 | function6 |
| I²C SDA | 11 | function6 |
| FUSB302 INT_N | 16 | input pull-up, GPIO IRQ/level waits |
| Button A / B | 25 / 28 | input pull-down |

У IronOS `Pins.h` SCL/SDA подписаны наоборот. Его `pinmux_config.h` на обоих
пинах включает function6 и потому не меняет действительную аппаратную разводку.
Это расхождение особенно существенно для будущего GPIO bus recovery.
GPIO23..28 учитывают `GPIO_USE_PSRAM__IO`: SDK может перенаправлять pad config на
индексы 32..37. Функции других выводов паяльника автоматически не настраиваются.

## Примеры

- `heartbeat`: UART banner, счётчик/время раз в секунду, кнопки и уровень PD_IRQ.
- `fusb302`: `write_read(0x22, [DEVICE_ID=0x01], 1 byte)` раз в секунду, печать Result.
  Не читает read-to-clear INTERRUPT, не пишет RESET, не ведёт USB-PD negotiation.
- `i2c_latency`: чтение девяти readable registers FUSB302 (0x01..0x09) с блокировкой
  executor на 20 ms внутри первого Pending poll, при включённых IRQ. Передача
  больше FIFO должна продвигаться из ISR. Пример пока только собран.
- `fpu`: IMFC f32 result, FS/fcsr, Timer.
- `gpio_irq`: три независимых GPIO waiters и UART reporter: кнопка A any-edge с
  timeout/cancel/rearm, кнопка B rising/falling, FUSB302 INT_N low/high.
  Без debounce и без очистки FUSB302 interrupt registers; подробности [gpio.md](gpio.md).
- `atomics`: отдельные однократные probes собственной RAM, см. [atomics](atomics.md).
- `analog`: ADC scan CH10/9/8, TIMER1 compare, unrouted PWM0 period IRQ, лог trim и samples.
- `sampling`: TIMER0 ISR владеет ADC и unrouted PWM1; три compare-фазы, latest frame
  через Signal. GPIO21 остаётся low; это dry-run без нагревателя, см. [analog.md](analog.md).

Read-only probe намеренно не проверяет ID на одно фиксированное значение:
варианты FUSB302 и FUSB302B отличаются. Зафиксировать фактическую маркировку U1 и ID.

## Порядок аппаратной проверки

Для первого bring-up снять жало, использовать питание с ограничением тока и
проверить низкий уровень Tip_Control. Подготовить backup штатной flash и способ
возврата в ROM ISP. RAM ELF не является разрешением или инструкцией стирать flash.
Этот проект никаких hardware writes автоматически не выполняет.

После подготовки подходящего JTAG adapter/OpenOCD target-конфига пример ручной
RAM-загрузки в GDB выглядит так; эти команды здесь **не выполнялись**:

```gdb
file target/riscv32imc-unknown-none-elf/release/heartbeat
target extended-remote localhost:3333
monitor reset halt
load
set $pc = _start
break bl706_fatal_trap
continue
```

Если отладчик не может загрузить ITCM/OCRAM после reset, сначала установить его
memory/reset contract. Не заменять это произвольной записью flash/cache registers.

1. **Reset/RAM:** проверить debugger ID и CSR, записать ROM/SiFive revision,
   загрузить ELF, убедиться в entry `0x22010000`, stack в OCRAM, обнулении bss,
   сохранении `.data`. Отдельно проверить после cold reset и debugger reset.
2. **Clocks/UART:** пройти clock transition пошагово; прочитать HBN_GLB и dividers.
   Измерить UART bit time/8N1. Проверить timeout при остановленном UART и отсутствие
   зависания logging. RC timing сверять внешним прибором, не собственным mtime.
3. **Trap/CLIC:** вызвать контролируемое исключение, проверить BL706_TRAP.
   Прочитать mtvec/mtvt, включить MTIME и проверить возврат через mret. Убедиться,
   что не возникают непрерывные pending IRQ после обслуживания.
4. **Time/executor:** heartbeat ≥10 минут без пропусков/застреваний. Несколько tasks
   с разными deadline, уже истёкший deadline, перепрограммирование на более ранний,
   cancellation и повторное использование waker. Принудительно поставить mtime
   около rollover low32 и проверить hi/lo/hi чтение. Измерить дрейф RC.
5. **GPIO:** проверить кнопки и PD_IRQ полярность, SF pad remapping для GPIO25/28.
   Изменения OE не должны давать импульс heater. Запустить `gpio_irq`, проверить
   IRQ 60 и возврат, rising/falling/any-edge обоих кнопок, несколько simultaneous
   pins, timeout/cancel/rearm без stale completion. Уже активные low/high должны
   завершаться сразу, edges ждать нового фронта. Проверить оба детектора и CLR
   pulse осциллографом/чтением регистров. Удержание INT_N low не должно создавать
   IRQ storm или мешать timer/UART. Для FUSB302 достаточно наблюдения; read-only
   пример не снимает latched INT_N. На отдельном свободном входе проверить внешние
   импульсы разной ширины и latency при I²C/mtime IRQ; не подавать их на линии heater.
6. **I²C analyzer:** для DEVICE_ID увидеть START, 0x44(W), 0x01, repeated START,
   0x45(R), byte, master NACK и STOP. Проверить именно GPIO10=SCL/GPIO11=SDA,
   фактическую скорость около 100 kHz, clock stretching и отсутствие переполнений.
7. **I²C failures:** NACK на заведомо незанятом адресе; удержание SDA/SCL; timeout;
   отмена future посреди передачи; затем удачная новая транзакция. Проверить, что
   после drop отключены CLIC/I²C masks, m_en=0, FIFO очищены. При физически зажатой
   линии controller cleanup не заменяет bus recovery. При error/cancel внешний
   RX-буфер должен оставаться неизменным. Никаких обещаний auto-recovery.
8. **FIFO/transactions:** на внешнем тестовом slave прогнать read/write lengths
   1,2,3,4,5,7,8,9,255,256; sub-address 1..4; проверить порядок байтов и отсутствие
   padding на проводе. Unsupported shapes/zero/257 не должны начинать обмен.
   Проверить data NACK, ARB loss и FIFO error при возможной инъекции.
9. **Concurrency:** timer task продолжает исполняться во время I²C/timeout.
   Проверить completion до регистрации waker, IRQ между polls, late END/final RX
   word и stop transfer после cancel. Запустить `i2c_latency`: ожидать stalled=true,
   успешный обмен и корректные девять байт на analyzer, несмотря на 20 ms без
   polling future. При намеренной задержке waiter аппаратная операция должна
   завершиться и отключить IRQ/controller непосредственно в ISR. Отдельно
   измерить задержку ISR, влияние других ненестированных IRQ и critical sections.
   Длительный UART polling блокирует cooperative executor, но при MIE=1 не должен
   мешать FIFO service. Он по-прежнему может задерживать PD policy и timers tasks.
10. **FPU:** проверить FS/fcsr и битовый результат примера. Под отладчиком заполнить
    все FPR различимыми sentinel, вызвать MTIME/I²C IRQ с изменением FP-регистров
    внутри handler и сравнить f0..f31/fcsr/mstatus после mret. Проверить stack watermark.
11. **Atomics:** отдельный binary, записать результаты и traps обеих probes,
    не включать A-target по результату одной успешной операции. Проверку других
    aliases/памяти/DMA проектировать отдельно; произвольные MMIO не использовать.
12. **TIMER:** в `analog` проверить IRQ 53 и независимость `mtime`, затем IRQ 52 в
    `sampling`. Прочитать shared TCCR/TCDR/TCER/TCMR до/после настройки второго канала,
    убедиться, что первый канал и watchdog fields не изменились. Capture request
    должен давать меняющийся count. Измерить реальные compare/preload интервалы,
    включая SDK offset в два тика, stop/restart, rollover, cancel/rearm, pending до
    waker. 32K/1K отдельно проверять только после установления их clock readiness.
13. **ADC:** сначала GND с `Adc<()>`, затем CH8/9/10 с известным напряжением и
    измерением мультиметром. Проверить FIFO channel tags (negative=23), порядок
    восьми позиций, 12/14/16-bit alignment, короткие scans 1..12, timeout/cancel/reuse,
    overrun/underrun/saturation. Зафиксировать доступность boot shadow, enable/parity
    trim и различие raw/calibrated readings; не считать отсутствие ошибки калибровкой.
    Проверить 100 us restart pause, analog settling, reference и влияние нагрева
    только в отдельном, явно подготовленном аппаратном тесте. Здесь нагрев не включать.
14. **PWM:** начать с unrouted counter/IRQ 49. Для waveform выбрать отдельный
    проверенный pad/fixture, не GPIO21. Проверить divider/period, duty 0/50/100%,
    enable/disable/drop-low, STS_TOP timeout и period IRQ clear 1→0. Одновременные
    каналы/IRQ не должны менять соседние masks; cancel не должен отключать чужой PWM.
15. **Dry-run sampling:** при GPIO21 low проверить три фазы TIMER0/ADC и выход frames.
    Задержать cooperative task, оставив MIE=1: ISR должен продолжать цикл, Signal
    может пропускать frames. Отдельно задержать IRQ до coalesced compare или ошибки
    ADC и проверить остановку dry-run. Измерить latency и stack watermark под
    одновременной нагрузкой I²C/GPIO/mtime; эта проверка ещё не является допуском
    к реальному управлению нагревателем.

Ни один пункт этого списка пока не выполнен на железе. Для дальнейшего обычного
USB flashing отдельно потребуются boot header, flash parameters, boot2/ROM contract,
XIP/cache setup, linker layout и проверенный путь упаковки образа.
