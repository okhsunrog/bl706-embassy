# GPIO и async IRQ

`Input<N>` реализует `embedded_hal::digital::InputPin` и
`embedded_hal_async::digital::Wait`. Все пять ожиданий используют GPIO IRQ,
таймерного опроса нет:

```rust,ignore
let mut int_n = p.pins.p16.into_input(bl706_hal::gpio::Pull::Up);
int_n.wait_for_low().await;
// Обслужить источник и снять INT_N перед следующим ожиданием low.
```

`wait_for_low/high` завершается сразу, если соответствующий уровень уже есть.
`wait_for_rising_edge/falling_edge/any_edge` ждёт новое событие после настройки
детектора на первом poll; старый pending очищается. События до arming не сохраняются.
`Wait` возвращает `Infallible`; каждый `Input` допускает одно ожидание благодаря
`&mut self` и исключительному владению пином.

## Схема Embassy

Сверено с Embassy commit `4c0da6097c5b3ce078473016d7c4654f90693afa`:

- [embassy-rp GPIO](https://github.com/embassy-rs/embassy/blob/4c0da6097c5b3ce078473016d7c4654f90693afa/embassy-rp/src/gpio.rs):
  waker на пин, очистка старых edges перед arming, ISR маскирует источник и будит task.
- [embassy-stm32 EXTI](https://github.com/embassy-rs/embassy/blob/4c0da6097c5b3ce078473016d7c4654f90693afa/embassy-stm32/src/exti/mod.rs):
  per-line wakers, one-shot masking, cancel cleanup. Отдельный ExtiInput нужен STM32
  из-за разделяемых EXTI channels между портами. В BL706 такой межпортовой
  конкуренции нет, поэтому ожидания встроены прямо в `Input`, как у RP.

Здесь per-pin wakers и битовые `armed/complete` находятся в статической памяти
под одной critical section. ISR не хранит указателей на `Input` или future.
Регистрация waker и проверка completion сериализованы с ISR; событие до регистрации
waker остаётся в completion. Сработавшие пины маскируются **в обоих детекторах**,
статус очищается, затем их wakers извлекаются и вызываются вне заимствования state.
Один IRQ может завершить несколько независимых ожиданий.

Drop future (включая `with_timeout`/select) маскирует оба детектора этого пина,
очищает pending/completion и освобождает waker. Другие GPIO продолжают ждать.
Drop самого `Input` делает ту же очистку. Забытый future не оставляет висячих
указателей: ожидание остаётся вооружённым до события, повторного arming или drop Input.
На событии оно одноразово маскируется. Повторный wait очищает его старое состояние.

## Регистры BL706

Основание: [RM GLB](https://github.com/bouffalolab/bl_docs/blob/665e7daa84b498a767e656f458f26aca3be4fd45/BL702_RM/en/RST/GLB.rst),
[таблицы регистров](https://github.com/bouffalolab/bl_docs/blob/665e7daa84b498a767e656f458f26aca3be4fd45/BL702_RM/en/RST/glb_register.rst),
[SDK GLB](https://github.com/bouffalolab/bouffalo_sdk/blob/237ed937856bf435f4f3642ea4728294723e1d87/drivers/soc/bl702/std/src/bl702_glb.c)
(`GLB_Set_GPIO_Int[2]Mod`, `GPIO_INT0_IRQHandler`) и
[LHAL GPIO](https://github.com/bouffalolab/bouffalo_sdk/blob/237ed937856bf435f4f3642ea4728294723e1d87/drivers/lhal/src/bflb_gpio.c).

- Общий CLIC source `GPIO_INT0`: 16 + 44 = **60**, vector/dispatcher установлен HAL.
  HAL init маскирует все GPIO-источники, очищает оба детектора и включает CLIC line.
  Глобальный MIE по-прежнему включает приложение после init.
- Первый детектор: MASK `0x1a0`, STAT `0x1a8`, CLR `0x1b0`, MODE `0x1c0..0x1cc`.
  Второй: MASK `0x1d0`, STAT `0x1d4`, CLR `0x1d8`, MODE `0x1dc..0x1e8`.
  MASK=1 запрещает источник. Оба STAT RO. Оба CLR RW, SDK пульсирует 1 → 0;
  PAC исправлен соответственно, это не W1C.
- По десять **логических** GPIO на MODE word, три бита на пин. RMW под CS сохраняет
  соседние конфигурации. SF/PSRAM remap относится к pad config, не к номеру IRQ bit.
- Выбран SDK async control: falling=4, rising=5, low=6, high=7.
  `any_edge` включает rising в первом и falling во втором детекторе. SDK ISR
  обслуживает оба STAT в одном GPIO_INT0. Их независимость и маршрутизация ещё
  требуют аппаратной проверки. Полярность после чтения уровня не переключается.
- Для level wait есть повторное чтение уровня после unmask. ISR сначала маскирует
  источник, поэтому удерживаемый low/high не должен создавать IRQ storm.

## Границы проверки

Семь host-тестов исполняют production state engine с моделью регистров: все 32
mode fields, направления edges, latched completion до poll, уже активные levels,
гонка level/unmask, оба детектора, несколько пинов, cancel/stale IRQ/rearm.
Модель не исполняет target MMIO/waker/future Drop glue и не доказывает электрическую работу GPIO, длительность CLR pulse, аппаратную
latency, минимальную ширину импульса или фактический возврат из IRQ 60.

Это одноразовое уведомление, не счётчик всех фронтов. Импульсы между ожиданиями
теряются; дребезг не фильтруется программно. Async control не обещает deep-sleep
wakeup: executor пока активный, HBN/PDS GPIO wakeup не реализован.

Пример `gpio_irq`: GPIO25 any-edge + timeout/rearm, GPIO28 rising/falling,
GPIO16 FUSB302 low/high и независимый UART/timer reporter. Он наблюдает INT_N,
но не очищает FUSB302 interrupt registers и не реализует PD policy. Если INT_N
застрял low, task остаётся на `wait_for_high`, остальные задачи продолжают работать.
Без debounce счётчики кнопок могут увеличиваться несколько раз за нажатие.
На плате пример ещё не запускался; порядок проверки см. [hardware.md](hardware.md).
