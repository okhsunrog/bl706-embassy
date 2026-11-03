# PAC: сравнение и границы аудита

Генерация: официальный `soc702_reg.svd` → `patch_svd.py` → chiptool →
`finish_pac.py` → rustfmt. Все версии входов сохранены; SHA и pinned links в [sources](sources.md).

## Что изменено у andelf

Машиночитаемый результат: [svd-comparison.json](svd-comparison.json).
`tools/check_svd.py` сравнивает XML по именам, нормализует числовые записи и исключает
пустые XML-узлы из семантического сравнения.

Между официальным SVD и исходным andelf:

- добавлен RF-блок с 141 регистром на `0x40001000`;
- удалён неполный `<cpu>`; изменены имя, vendor/description и добавлена версия;
- пустая база AON заменена на `0x4000F000`;
- остальная карта регистров совпадает. В частности, неверные базы DMA/KYS сохранены.

YAML-патч andelf заменяет два 16-битных поля `cgen_cfg1` 32 именованными gates,
меняет описания блоков. Числовые и пустые XML-узлы дополнительно переформатированы svdtools.
GPIO arrays в YAML **закомментированы**, поэтому не являются применённым исправлением.
Патч descriptions для AON/HBN не проявляется в результате: source names имеют верхний регистр,
а YAML использует нижний. Сравнивается и YAML, и фактический `.svd.patched`.

## Принятые исправления

| Исправление | Основание | Проверка |
|---|---|---|
| Имя BL706, версия, удаление неполного CPU | chiptool/svd parser отвергает CPU без revision; ISA берётся из Rust target, не этого XML | parsing/generation |
| AON `0x4000F000` | SDK `bl702.h` AON_BASE, register offsets внутри блока | совпадает с andelf и SDK |
| DMA `0x40007000` → `0x4000C000` | RM §1 memory map; SDK DMA_BASE | XML assert + PAC const |
| KYS `0x4000AB00` → `0x4000A900` | RM memory map, SDK KYS_BASE | XML assert + PAC const |
| UART1 `0x4000A100`, QDEC1/2 `0x4000A840/880` | RM memory map и SDK | одинаковая layout с исходным экземпляром |
| UART0/1/I²C/GPIO IRQ 45/46/48/60 | SDK IRQ_NUM_BASE=16; RM источники 29/30/32/44 | XML, HAL CLIC map |
| cgen_cfg1 gates UART0=16, UART1=17, SPI=18, I²C=19, PWM=20, TIMER=21, IR=22 | SDK `GLB_AHB_Get_Clock` / `UnGate` | именованные setters, XML asserts |
| GPIO input/status RO, clear WO; I²C FIFO data RO/WO; UART status/RX RO, TX/clear WO | RM register access, SDK headers | типы `Reg<...,R/W>` |

Остальные gates сохранены сгруппированными. У andelf KYS обозначен bit25, тогда как
современный SDK использует bit27; bits24–26 связаны также с QDEC/I²S. Без нужды для
минимального HAL эти поля не переименованы и не используются. `MAX` bit31 и описания
`CCI/L1C (...efuse?)` не перенесены как достоверные аппаратные факты.

RF из andelf не импортирован: это существенное дополнение без проверенной здесь
происходящей от производителя спецификации. Его наличие в andelf не подтверждает
корректность каждого поля. Исходный официальный SVD остаётся источником генерации.

## Проверенные пути HAL

- `GLB_CLK_CFG0/2/3`, HBN_GLB root/UART selection и `CPU_CLK_CFG`: сравнение с
  `GLB_Set_System_CLK[_Div]`, `GLB_Set_UART_CLK`, `GLB_Set_I2C_CLK`, `GLB_Set_MTimer_CLK`.
- GPIO packed config: два pad на word, IE/SMT/pull/func; data и OE offsets
  `0x180/188/190`; SF/PSRAM pad remapping для GPIO23..28 по `GPIO_USE_PSRAM__IO`.
- UART0: divider, free-run, 8N1, FIFO count и byte data, signal mux.
- I²C: packet length **N−1**, sub-address **N−1** и LSB-first, repeated START,
  FIFO **два 32-битных слова**, END/NAK/ARB/FIFO error и mask/enable/W1C в одном регистре.
- Core-local CLIC/mtime отсутствуют в SVD. Адреса берутся из SDK `e24/clic.h`
  и сверены с SiFive manual. Это небольшой ручной слой HAL, не выдаваемый за SVD-генерацию.

## Что не утверждается

Это не полный аудит всего SoC PAC. ADC, DMA channels, crypto/eFuse, USB, RF,
camera/MJPEG и flash controller в HAL не используются; их поля и reset values
полностью не сверялись. Наличие аксессора не подтверждает применимость блока к
конкретному package/варианту BL706. В частности, исходный SVD содержит MJPEG и ef_data_1,
которые нельзя считать проверенными функциями Pinecil.

Многие source registers наследуют RW и глобальный reset=0, даже если реальные
поля имеют смешанные права/ненулевые reset values. Исправлены access только у
перечисленных путей. `Default` fieldset chiptool равен нулю, а не обещанному silicon reset.
Для mixed status/control I²C HAL использует явные записи с нулевыми W1C при unmask;
универсальный read-modify-write таких регистров применять нельзя.

Upstream chiptool генерирует Cortex-M InterruptNumber и vector glue даже для RISC-V.
Скрипт удаляет именно этот glue с assertions количества совпадений. Enum IRQ и MMIO
остаются сгенерированными. CLIC runtime реализован отдельно. Изменение формата
chiptool потребует явного обновления скрипта, а не молчаливого regex-пропуска.
