# ADC, PWM и TIMER

Поддержка начальная, без запуска на BL706. Время и напряжения ниже номинальные.
Оба новых примера удерживают GPIO21 как обычный output low; PWM не подключён к pad.
Настройка полноценного регулятора нагрева, PD policy и его аппаратная проверка
в эту поддержку не входят.

## Владение и Embassy

`Peripherals` выдаёт один `AdcToken`, `TimerToken<0/1>` и пять `PwmToken<0..4>`.
Конструкторы потребляют токены; ADC также владеет analog pads, PWM — выбранным
output pad либо `Unrouted`. TIMER не разделяется на три независимых таймера:
три compare используют общий счётчик. Общие TCCR/TCDR/TCER/TCMR и clock gates
меняются под critical section с сохранением соседнего канала и watchdog fields.
Блок TIMER целиком не сбрасывается. Watchdog не реализован.

`embassy_time` продолжает использовать core-local `mtime`; TIMER0/1 ему не отданы.
Async API требует включённого глобального MIE. Wakers и IRQ state статические;
обработчики не сохраняют адреса future или caller buffers. Результат IRQ до
регистрации waker остаётся latched. Завершение/отмена маскируют свой источник,
очищают status и освобождают waker. CLIC sources: ADC=25, PWM=49, TIMER0=52, TIMER1=53.
Нестирования IRQ нет; длинный обработчик задерживает остальные периферийные IRQ.

## ADC

Поддержаны single-ended finite scans из 1..12 позиций, включая повторения каналов.
Входы: GPIO18/CH8, GPIO19/CH9, GPIO20/CH10 и внутренний GND/CH23. Аналоговые пины
переключаются на function10, без IE/SMT/pulls/OE. Тип `Inputs` допускает отдельный
analog pin, пару, тройку, вложенные tuples или `()` для внутреннего GND. Нельзя
сканировать внешний канал, соответствующий pad которого не передан в ADC.
Остальные внешние входы, differential, TSEN/VBAT, audio, continuous scan и DMA
пока не представлены публичным API.

```rust,ignore
let adc_inputs = (p.pins.p18.into_analog(), p.pins.p19.into_analog(),
                  p.pins.p20.into_analog());
let trim = GainCalibration::from_efuse_shadow();
let mut adc = Adc::new(p.adc, adc_inputs, p.clocks, Config {
    calibration: trim.ok(), ..Default::default()
});
let mut samples = [Sample::default(); 3];
adc.read_scan(&[Channel::Input8, Channel::Input9, Channel::Input10],
              &mut samples, Duration::from_millis(100)).await?;
```

ADC clock фиксирован: XCLK RC32M / GLB 4 / внутренний ADC 4 = nominal 2 MHz,
не выше указанного в RM максимума. Gates GPIP (`cgen_cfg1[2]`) и HBN GPADC включаются.
Выбор XCLK и div берётся из текущего контракта `Clocks`, PLL не используется.
Аналоговая конфигурация: V18=1.82V, V11=1.1V, PGA1/PGA2 x1, chop=2, main bandgap,
VCM=1.2V, delay=2, offset correction off. Это соответствует Pinecil BSP. Современный
LHAL дополнительно включает MIC2_DIFF/NEG_GND и выполняет другие калибровки; здесь
эта последовательность не копируется вслепую: IronOS отдельно оставляет MIC2_DIFF
выключенным. Результирующая точность и поведение именно BL706 требуют платы.

Доступны 12-bit, 14-bit average16/64, 16-bit average128/256. Это результаты
усреднения 12-bit SAR, не обещание физической 16-bit точности.
FIFO word содержит sample[15:0], negative tag[20:16], positive tag[25:21].
`raw` сохраняет левое выравнивание; `counts` выровнены вправо. Драйвер проверяет
порядок positive tags и negative tag=GND, а также лишние данные, overrun/underrun
и analog saturation. Требование negative tag=23 необходимо подтвердить на железе.

`GainCalibration::from_efuse_shadow` только читает boot-loaded shadow и включает
AHB gate EF_CTRL. Не запускает reload, ROM API и не пишет EF_CTRL/EF_DATA/fuses.
Если shadow не готов, busy или не переключён на SAHB, возвращается
`ShadowUnavailable`; отсутствие trim/неверная parity даёт `InvalidCalibration`.
Поле берётся из EF_DATA_0 + 0x78: trim[12:1], parity[13], enable[14].
Целочисленная формула соответствует SDK: `counts * 2048 / (2048 - signed_trim12)`,
с насыщением до максимума выбранного разрешения. `calibration=None` означает
некалиброванные отсчёты и отражается в `Sample.calibrated=false`. Результат
проверки shadow нужно логировать; fallback не является подтверждением калибровки.
`microvolts()` оценивает напряжение на ADC pin и не учитывает делитель VIN,
усилитель термопары, NTC или погрешность reference.

В async scan IRQ готовности FIFO (threshold=1 word) обслуживает до 12 результатов
за вход. Он сам останавливает ADC и маскирует IRQ до публикации завершения.
Future только ждёт и копирует результаты после успеха; caller output не меняется
при error/timeout/cancel. Ошибки проверяются до и после FIFO drain.
GPIP status/control смешаны: записи mask не повторяют прочитанные W1C flags.
FIFO_CLR и error clears записываются только явно при cleanup.
RM в описании scan упоминает CONT_CONV_EN=1; SDK и IronOS используют SCAN_EN=1
с CONT_CONV_EN=0 для конечного скана. Здесь выбран этот SDK-путь; число и порядок
результатов такого скана ещё требуют аппаратной проверки.

Для ISR-driven цикла есть `prepare_scan`, `start_prepared`, `stop`, `try_read`.
`prepare_scan` останавливает ADC и очищает FIFO. `start_prepared` не ждёт: вернёт
Busy, если после подготовки/stop ещё не прошло 100 us или ADC уже запущен.
Эта пауза взята из SDK `ADC_RESTART_DUMMY_WAIT`; async scan выдерживает её через
Embassy timer. Общий restart gate обновляется на каждом фактическом STOP, включая
IRQ completion/error и drop/cancel, поэтому переход к ручному запуску не обходит
паузу. `stop` сохраняет FIFO для drain владельцем, `abort` очищает FIFO
и async state, в том числе после забытого future. Забытый async scan без `abort`
занимает engine до аппаратного завершения. Если future забыли во время 100 us
паузы до START, операция сама не начнётся и engine останется Busy до `abort`.
Drop ADC останавливает его и снимает GLOBAL_EN. Pins не возвращаются автоматически.

## PWM

Все пять каналов независимы. Поддержаны BCLK, actual divider 1..65535 и period
1..65535, normal polarity и abrupt stop. Частота = BCLK / divider / period,
без скрытого +1. Канал маршрутизируется function8, GPIO modulo5 = channel;
использовать можно только действительно доступный pad конкретной платы.
`Unrouted` позволяет проверять счётчик и IRQ без физического выхода.

Реализован `embedded_hal::pwm::SetDutyCycle`: max duty = period; duty вне диапазона
отвергается, 0/100% задаются SW force. Новый PWM отключён и forced-low ещё до
подключения pad; `enable()` явный. `disable()` сначала принудительно выставляет
low, затем STOP_EN. Drop дополнительно маскирует IRQ и возвращает pad в GPIO low.
`configure` отключает PWM и ждёт STS_TOP до nominal 1 ms, дополнительно ограничивая
число polls на случай неисправного mtime; IRQ при этом
разрешены. Живое изменение duty не обещает фазово-синхронного/glitch-free обновления.

`wait_periods(n)` ждёт следующий hardware period-count IRQ с threshold=n,
не меняя duty. Фаза счётчика при arming не сбрасывается: это не точная задержка
n полных циклов от вызова. Общий IRQ обслуживает каналы независимо и одноразово
маскирует сработавшие. Clear требует pulse 1 → 0; перед нулём делается до 32
status reads по SDK. Если статус не снялся, waiter получает Timeout.
Отмена ожидания не выключает сам PWM, но маскирует IRQ и удаляет waker.
Забытый future не держит ссылок на caller storage; IRQ остаётся one-shot.

## TIMER

32-bit counter, actual divider 1..256, raw compare[3], free-running или preload
по одному выбранному compare. API содержит System/32K/1K sources; настроенный
текущей clock initialization источник — nominal System=32MHz. Low-frequency
sources представлены по RM, но их готовность и фактическая частота здесь не
устанавливались; `clock_hz()` возвращает nominal 32000/1000, не измеренное значение.
XTAL source пока не выставлен, поскольку HAL не запускает crystal.

Compare пишутся как переданы. SDK init автоматически вычитает два тика в ряде
случаев, а setter пишет значение без поправки. Здесь неоднозначность не скрыта:
реальные интервалы и preload boundary нужно измерить. `counter` повторяет SDK
capture request + три чтения TCVWR; RM помечает этот регистр RO, SDK требует write.
В PAC он оставлен RW с явным объяснением. Остановка не обещает обнуления счётчика;
счётчик/preload/restart semantics требуют HIL. Обновление compare уже позади
текущего count может ждать до rollover; драйвер не выдаёт его за немедленный match.

Обычный `Timer<N>` позволяет start/stop/configure/set_compare/listen/take_events.
Без custom ISR обработчик маскирует сработавшие compare и защёлкивает events.
`wait_for_compare` ждёт новое совпадение работающего counter; старый pending
сбрасывается при arming. Cancellation маскирует свой compare, но не останавливает
счётчик. Несколько одинаковых совпадений coalesce, это не очередь всех событий.

Альтернатива: `InterruptDriver<N,H>` потребляет Timer и `H: Handler<N>`.
Экземпляр размещается в StaticCell и `register(&'static mut self, events)` навсегда
передаёт владение runtime. ISR подтверждает события, затем вызывает
`H::on_interrupt(&mut self, &mut Timer<N>, Events)` непосредственно в IRQ.
ADC/PWM можно хранить в H и управлять ими без пробуждения executor. Обычного Timer
у приложения после register нет, поэтому два обработчика не делят один канал.
Callbacks не должны логировать, блокироваться или ждать другой IRQ. Pending всех
трёх compare передаётся одним Events: порядок пропущенных фаз нельзя восстановить
по этим битам. Не следует выдумывать правильный порядок при перегрузке.

## Примеры и границы

- `analog`: async ADC scan из восьми позиций, TIMER1 compare и PWM0 period IRQ
  с timeout; PWM0 unrouted, GPIO21 low. Печатает статус gain trim и сырые samples.
- `sampling`: статический ISR driver владеет TIMER0, ADC и **unrouted PWM1**.
  Compare0=50000: выключить PWM counter output; compare1=51000: запустить ADC;
  compare2=60000: stop/drain, подготовить следующий scan и возобновить unrouted PWM.
  Clock nominal 1MHz, preload по C2. Coalesced/неожиданные события, просроченная фаза
  или ошибка ADC останавливают этот dry-run driver. Отдельная task получает latest
  Frame через Embassy Signal и логирует. Signal может заменять непрочитанный frame,
  это не lossless поток. Нагреватель физически остаётся GPIO low во всём примере.

Host-тесты проверяют production ADC engine, все длины scan 1..12, ошибки перед и
после drain, cancel/reuse, scan packing, resolution, все signed12 trim/parity,
границы PWM и TIMER и сохранение соседних register fields. MMIO, actual FIFO tags,
W1C side effects, callbacks/waker glue, точность и latency IRQ, analog settling,
PWM pad waveform и fail-off timing этими тестами не подтверждены.
`sampling` не заменяет регулятор: отсутствуют command freshness cutoff, PD/power
limits, thermocouple conversion и проверенная аппаратная защита. План HIL —
[hardware.md](hardware.md).

## Источники

- [RM ADC](https://github.com/bouffalolab/bl_docs/blob/665e7daa84b498a767e656f458f26aca3be4fd45/BL702_RM/en/RST/ADC.rst),
  [ADC registers](https://github.com/bouffalolab/bl_docs/blob/665e7daa84b498a767e656f458f26aca3be4fd45/BL702_RM/en/RST/adc_register.rst),
  [PWM](https://github.com/bouffalolab/bl_docs/blob/665e7daa84b498a767e656f458f26aca3be4fd45/BL702_RM/en/RST/PWM.rst),
  [TIMER](https://github.com/bouffalolab/bl_docs/blob/665e7daa84b498a767e656f458f26aca3be4fd45/BL702_RM/en/RST/TIMER.rst).
- [Official bl_iot_sdk](https://github.com/bouffalolab/bl_iot_sdk/tree/edcf613f073d3ef7d2f290d7c285b69be35da75b/components/platform/soc/bl702/bl702_std/BSP_Driver/std_drv):
  ADC restart/scan/trim, PWM stop/clear, TIMER capture/divider/compare.
- [SDK LHAL](https://github.com/bouffalolab/bouffalo_sdk/tree/237ed937856bf435f4f3642ea4728294723e1d87/drivers/lhal/src):
  bflb_adc.c, bflb_pwm_v1.c, bflb_timer.c; clock gates в bl702_glb.c.
- [IronOS Setup/IRQ](https://github.com/Ralim/IronOS/tree/c7413e0d2adfd43156510d4ccbd109cf352ca335/source/Core/BSP/Pinecilv2):
  PGA/chop/averaging, ADC pins, PWM1, три фазы TIMER0. Новый код не копирует
  firmware policy IronOS и не объявляет её перенесённой.
