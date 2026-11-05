# RTOS-independent BL70x BLE port

The `experiment/ble-controller` branch adds `crates/bl706-ble`. Its matching
[Pinecil experiment](https://github.com/okhsunrog/pinecil-rs/tree/experiment/ble-controller)
is a standalone heater-off image running Trouble 0.8 and bt-hci 0.10.1.
Normal application code remains on main.

## Binary and ABI boundary

The original controller and matching RF archives are stored unchanged under
`crates/bl706-ble/vendor`, with source links, license and hashes. The build
verifies SHA256 and removes only `arch_main.o` from an OUT_DIR copy. It rejects
remaining direct FreeRTOS task/queue imports. No fake FreeRTOS functions or
ROM FreeRTOS symbol addresses are supplied.

Rust replaces the native eight-byte task message queue with a bounded
20-entry queue and a latched Embassy signal. Host messages retain vendor
allocation ownership until `hci_send_2_controller` consumes them. Firmware
scheduling notifications carry no owned payload and may coalesce. The worker
calls `rwip_schedule` synchronously, then awaits another notification.

IRQ72 dispatches the native radio handler. HCI callbacks immediately copy event
payloads; ACL callbacks immediately extract exchange memory before the native
message is freed. RX has 16 owned packets. Overflow fails loudly instead of
silently losing command completions or ACL credits. Command credits and the
standard Controller/ControllerCmd traits come from bt-hci's ExternalController.
The transport supports commands and ACL, not ISO or synchronous data.

ABI sizes/offsets are compile-time assertions. Packet conversion and native
connection-handle routing have host tests. The selected archive exports the
correctly spelled `bt_onchiphci_handle_rx_acl`; the old debug reference uses
`hanlde`. Do not exchange these binaries without checking their exports.

The pinned nightly supports native Rust C variadics and `VaList::next_arg`.
The small RTT printf adapter uses `core::ffi`; it supports the conversions used
for bring-up and stops on unsupported conversions without consuming wrongly
typed arguments. It is not a general libc formatter. Native fatal reset requests
panic; they are not reported as successful no-ops.

## Clocks, memory and startup

Use a 32 MHz crystal reference; the current example uses the validated 144 MHz
CPU / 72 MHz bus clock profile. Early 32 MHz tests advertised and transferred
GATT data, but connection establishment also timed out, including without RTT
polling. The later 144 MHz image, also with pooled entropy, completed three
connect/read/write/notify/disconnect cycles without those failures. This is
bring-up evidence, not an isolated proof of the timeout's root cause.

Keep the controller's `.bleromro` in executable XIP and copy `.bleromrw`,
`.retention` and `.tcm_data` into initialized RAM. Zero `.retention_noinit` on
cold startup (the selected archive contains only zeros in that section).
The ELF audit must reject orphan vendor sections outside these copy/zero ranges.

RF calibration temporarily redirects software IRQ3 and uses
`0x42024000..0x42028000` as scratch. The standalone image puts live data, BSS
and its 8 KiB stack in ITCM `0x22014000..0x22020000`, leaving that OCRAM alias
free. The upper 8 KiB of OCRAM belongs to exchange memory; GLB_EM_8KB is encoded
as `0x3`. This is not a compatible linker layout for an arbitrary application.

The HAL's temporary IRQ3 hook is installed only while the caller exclusively
borrows the unused software-interrupt token. RF restores the hook before an
InterruptExecutor may start. Later attempts to register a calibration handler
fail explicitly. Radio IRQ72 is independent of that software executor.

TRNG is polled with a finite timeout in thread context and supplies a pool of
fresh words. IRQ-side random requests only pop a word in a short critical
section. There is no deterministic fallback on timeout, health failure or pool
exhaustion. This experiment owns the TRNG exclusively.

## Hardware evidence and remaining work

On 2026-11-05, with Pinecil V2 powered from the laptop over Type-A, the Pico
CMSIS-DAP JTAG probe downloaded and verified the XIP image. Native HCI Reset,
Read Local Version/Supported Commands/Supported Features, LE Read Buffer Size,
LE Read Local Features and Read BD_ADDR returned successful command completions.
The controller reports 251-byte ACL data buffers, two packets.

Trouble advertised `Pinecil Rust`. Linux/BlueZ and Bleak discovered the custom
service, read a u32 counter, wrote new counter values with response, verified
readback and received monotonically increasing notifications. Repeated
connections/disconnections exercised both native ACL directions. These are
laptop tests, not phone or qualification testing.

Timing instrumentation exposes per-window interrupt, synchronous scheduler and
firmware-wakeup-to-worker maxima. Initial live-JTAG measurements had millisecond
outliers; do not infer an application scheduling budget from them. Before
integrating OLED/PD, measure without debugger traffic and with real UI load,
then choose the worker's execution priority. The C scheduler has no async yield
inside a call.

Not implemented or validated: radio low-power sleep, pairing/bonding/encryption,
OTA, concurrent OLED/PD load, multiple connections or central role. The test
GATT counter never enables or controls heating.

## Offline checks

`tools/check.sh` includes packet tests, controller-target Clippy and the existing
HAL/PAC/IMC/IMFC checks. The application repository links and audits the actual
BLE image. A build or CI result alone does not establish a working radio link.
