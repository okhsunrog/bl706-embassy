# BLE controller port investigation

This is an experiment. No controller from this investigation has been linked
into the firmware or tested on Pinecil. The existing firmware keeps heating off.

## Intended roles

Pinecil is a BLE peripheral and GATT server; a phone is the central and GATT
client. Settings writes, telemetry notifications and a future OTA service all
fit this arrangement. OTA does not require Pinecil to become a central.

The initial candidate is `libblecontroller_bl702_m0s1.a`: one peripheral
connection, without the PDS power-down-sleep option. Scanning and central roles
are not required for this application. Role selection alone does not establish
which pairing, encryption or optional HCI operations are available.

## Sources compared on 2026-11-05

| Source | Archives inspected | DWARF `.debug_info` |
| --- | --- | --- |
| [Legacy official IoT SDK](https://github.com/bouffalolab/bl_iot_sdk/tree/edcf613f073d3ef7d2f290d7c285b69be35da75b), SDK label `1.6.39-238-gf5ba0a7ee` | `blecontroller_702_std` | Present in all 46 members |
| [IronOS Pinecil V2 SDK copy](https://github.com/Ralim/IronOS/tree/c7413e0d2adfd43156510d4ccbd109cf352ca335/source/Core/BSP/Pinecilv2/bl_mcu_sdk/components/ble) | `blecontroller_702_std` | Present in all 46 members |
| [Sipeed MCU SDK snapshot](https://github.com/sipeed/bl_mcu_sdk/tree/c0ed305c05ca089ab3debab9b1bd432d6cce9b10) | `blecontroller`, `blecontroller_702_std` | Present in all 46 members of each archive |
| [Official split components repository](https://github.com/bouffalolab/bl_iot_sdk-components/tree/a18c0f9215a9490ca2ce4811084b96ee60e4f205), imported 2026-05-26 | BL702 `m0s1`, `m0s1s`, `m1s1`, matching RF library | Absent; function/local symbols and relocations remain |

The current combined Bouffalo SDK does not supply a BL702 BLE controller in its
Bluetooth library directory. Its BL602/BL616/BL702L libraries are not substitutes
for BL702/704/706. The split components repository does contain BL702 libraries;
its controller variants have an embedded `lib_version_blecontroller_1.6.17`
marker. The repository import date is not the binary's compilation date.

The vendor [variant description](https://github.com/bouffalolab/bl_iot_sdk-components/blob/a18c0f9215a9490ca2ce4811084b96ee60e4f205/network/ble/README.md)
and [build configuration](https://github.com/bouffalolab/bl_iot_sdk-components/blob/a18c0f9215a9490ca2ce4811084b96ee60e4f205/network/ble/ble_common.mk)
distinguish roles and feature restrictions. In particular, disabling the SDK's
C host SMP/ECC code is not proof that a different Rust host cannot provide
pairing. The final host/controller security combination needs its own validation.

## Pinned initial binary pair

These are the pair selected by the
[Zephyr module manifest](https://github.com/zephyrproject-rtos/hal_bouffalolab/blob/18c998da7dcf550303bc9b2852602dcb401e21a1/zephyr/module.yml),
with both downloaded SHA256 values verified:

| Archive | File bytes | SHA256 |
| --- | ---: | --- |
| [`libblecontroller_bl702_m0s1.a`](https://github.com/bouffalolab/bl_iot_sdk-components/blob/a18c0f9215a9490ca2ce4811084b96ee60e4f205/network/ble/blecontroller_bl702_m0s1/lib/libblecontroller_bl702_m0s1.a) | 1,138,936 | `94d05d9c87b64703bb6b8c330ea6b21a5f40258e3301324741e9141be7e2a9b5` |
| [`libbl702_rf.a`](https://github.com/bouffalolab/bl_iot_sdk-components/blob/a18c0f9215a9490ca2ce4811084b96ee60e4f205/platform/soc/bl702/bl702_rf/lib/libbl702_rf.a) | 122,112 | `94dd54071072ea241367b094b109f97a85e731e301f94cce3946d89741893ad6` |

The legacy debug reference controller has SHA256
`031ee9bcdc4957738cc4709d88b8c70edb4c0cd4f495644ff3f448635313c4db`.
It is a separate version, not an interchangeable RF/controller pair.

These file sizes include symbol tables, relocations and other object metadata.
They are not application flash usage. A linked image must account for controller
RAM/ROM state and exchange memory as well as ordinary ELF sections and COMMON
symbols; summing archive `.bss` alone does not establish RAM usage.

## Evidence for an RTOS-independent port

All inspected controller variants have the same nine strong external FreeRTOS
imports, all referenced by `arch_main.o`:

```text
xTaskCreate             vTaskDelete          uxTaskPriorityGet
xQueueGenericCreate     vQueueDelete         xQueueReceive
xQueueGenericSend       xQueueGenericSendFromISR
vTaskSwitchContext
```

This establishes the location of direct unresolved references. It does not
prove that every indirect call or ROM implementation is RTOS-independent.
The [full `arch_main.o` analysis](ble-arch-main.md) identifies the small task and
queue wrapper and its ROM hooks.

The controller archives declare `rv32imfc` / single-float ABI, matching the
project's FPU target without A. The matching RF library also references software
f64 helpers such as `__muldf3`; the F extension does not implement f64 operations.
Actual retained instructions and helper linkage must be audited after linking.

## Existing port to study

Zephyr has a [BL70x HCI adapter](https://github.com/zephyrproject-rtos/zephyr/blob/76f604468505b30dfafa2c5a80d8b1c665ed6633/drivers/bluetooth/hci/hci_bflb.c),
[platform bridge](https://github.com/zephyrproject-rtos/zephyr/blob/76f604468505b30dfafa2c5a80d8b1c665ed6633/soc/bflb/bl70x/bflb_platform_shim.c)
and [FreeRTOS compatibility layer](https://github.com/zephyrproject-rtos/zephyr/blob/76f604468505b30dfafa2c5a80d8b1c665ed6633/modules/hal_bouffalolab/shim/freertos_shim.c).
That layer creates real Zephyr threads and blocking queues. It is useful for ABI
and packet-lifetime evidence, but cannot be copied into a stackless async task.

Important integration constraints:

- The on-chip API exchanges vendor command/ACL structures and callbacks, not an
  H4 UART byte stream. Callback data must be copied or extracted before return;
  the vendor may free its backing message immediately afterwards.
- The stock wrapper registers BLE IRQ72. The HAL's current vectors do not yet
  dispatch it.
- The stock integration uses GP-relative ROM state and software IRQ3. That IRQ
  already runs the HAL's interrupt executor; a port must preserve its operation.
- Clock/RF setup, MAC address, random numbers, exchange memory and ROM writable
  data remain separate requirements after removing task/queue calls.

## Implementation sequence

1. Replace the task/queue part of `arch_main.o` with an explicitly owned,
   bounded controller worker and ISR wakeups. Preserve the vendor scheduler and
   controller state machine; reconstruct and verify each exported ABI.
2. Audit remaining ROM calls and GP/data layout, RF initialization and IRQ
   handling. Link a minimal image and inspect unresolved imports and instructions.
3. On hardware, complete HCI Reset and read controller version, supported
   commands/features, buffer sizes and device address before using a host stack.
4. Implement the compatible `bt-hci` controller interface, with owned RX storage,
   cancellation rules, command credits and ACL flow control. Run a Trouble
   peripheral with one small GATT service.
5. Validate advertising, phone connection, writes and notifications, then repeat
   with PD/UI load. OTA and heating remain subsequent work.

Trouble accepts controllers implementing the
[`bt-hci` interface](https://github.com/embassy-rs/bt-hci); the
[Trouble documentation](https://embassy.dev/trouble/) describes this boundary.

## Reproducing the static archive audit

The Nix development shell supplies GNU binutils and LLVM. Given downloaded
archives, run:

```sh
nix develop
uv run --no-project tools/audit_ble_archive.py /path/to/controller.a /path/to/rf.a
```

The script subtracts symbols defined by other archive members before reporting
external imports, checks ELF attributes and counts DWARF sections. It never
executes the controller. Hardware success is not established by this audit.
