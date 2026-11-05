# `arch_main.o`: task, queue and ROM boundary

Static analysis on 2026-11-05 compares the legacy official `702_std` controller
with the split-components `bl702_m0s1` selected in
[the controller investigation](ble-controller.md). This is a reconstruction
from machine code, ELF relocations and old DWARF, not recovered vendor C source
or a working replacement implementation.

## Size and debugging information

| Property | Legacy official `702_std` | Split-components `m0s1` |
| --- | ---: | ---: |
| Object file bytes | 62,236 | 18,512 |
| Machine code, sum of `.text*` | 958 | 916 |
| Named function definitions, excluding compiler labels | 20 | 20 |
| `.debug*` section bytes | 17,008 | 0 |
| DWARF version | 4 | No DWARF |
| `.debug_info` bytes | 5,021 | 0 |

The old object records structure fields/offsets, parameter and local-variable
names, enum values, source paths/line numbers and compiler options. Its producer
string specifies GCC10.2, `-march=rv32imfc -mabi=ilp32f`, `-Os`,
`-ffunction-sections`, `-fdata-sections`, `-fcommon` and `-fshort-enums`.
It does not embed the original C source. Optimization also means not every
source variable has a recoverable runtime location.

The new object retains named functions, local labels, symbols and relocations,
but no DWARF types or line tables. Both are straightforward to disassemble.
All 46 members of the old controller have `.debug_info`; the three inspected
new controller variants have none. Thus “contains symbols” and “contains full
debug information” are different claims for these archives.

## The RTOS-dependent part is small

These seven functions contain all nine direct FreeRTOS imports in `m0s1`:

| Function | Code bytes | Behavior |
| --- | ---: | --- |
| `blecontroller_main` | 78 | Wait for a queue item, dispatch a host message, run `rwip_schedule`, repeat |
| `ble_controller_task_init` | 72 | Create a 20-item queue and a task with a 512-word stack |
| `rw_main_task_post` | 44 | Send a message with the caller's timeout; report success |
| `rw_main_task_post_from_fw` | 82 | Post a firmware wakeup from thread or ISR context |
| `rw_main_task_post_from_isr` | 58 | Post a firmware wakeup from ISR context |
| `rw_main_get_task_priority` | 38 | Query priority if the task exists |
| `ble_controller_deinit` | 124 | Disable BLE IRQ, delete task, drain/free messages, delete queue |
| **Total** | **496** | |

Their instruction bytes match the old debug-reference object. The global call
and data relocation targets agree as well; some local compiler-label names
differ. This makes the old DWARF a useful guide for this wrapper. It does not
establish equivalence of the rest of the controller.

The ABI recovered from old DWARF and checked against new loads/stores is:

```c
struct rw_task_msg {
    uint8_t msg_type;  /* offset 0 */
    /* three padding bytes */
    void *params;     /* offset 4 on RV32; total structure size 8 */
};
/* HOST_TO_FW_MSG = 1, FW_TO_FW_MSG = 2 */
```

Queue creation passes `(20, 8, 0)` to `xQueueGenericCreate`. Task creation passes
stack depth `0x200`, or 512 FreeRTOS words (2048 bytes on this port). This stack
size is a stock RTOS allocation, not a requirement for the eventual async worker.

The essential task behavior reconstructs as:

```c
/* Semantic pseudocode; this is not a port implementation. */
memset(&msg, 0, sizeof(msg));
for (;;) {
    if (xQueueReceive(queue, &msg, UINT32_MAX) == 1) {
        if (msg.msg_type == HOST_TO_FW_MSG)
            hci_send_2_controller(msg.params);
    }
    rwip_schedule();
}
```

The ISR notification functions enqueue `{FW_TO_FW_MSG, NULL}`. They call
`vTaskSwitchContext()` if `xQueueGenericSendFromISR` sets the higher-priority-task
woken flag to 1. `rw_main_task_post_from_fw` chooses the ISR path using
`TrapNetCounter`; its thread path sends with an infinite timeout.

An Embassy replacement must turn waiting into a pending future, preserve host
message ordering/ownership and reliably latch ISR wakeups. Returning success
from dummy queue functions would lose work. Calling the stock infinite task
loop from an Embassy task would block the executor.

During deinit, each queued non-null `params` is adjusted by minus12 before
`ble_ke_msg_free` is called. This is evidence of the vendor message-header ABI;
it must be verified with the allocator and HCI wrapper before reproducing any
pointer arithmetic in a port.

## Other responsibilities in the object

- `BLE_ROM_hook_init` installs five pointers in the 20-byte `.bleromrw` section:
  the ROM patch hook, memcpy, memset, memcmp and the firmware-post callback.
  The indirect memset calls at task entry/deinit use this table.
- `BLE_ROM_patch` currently returns zero.
- `ble_controller_init` installs hooks, sets a GLB bit at `0x40000028`, clears
  and registers BLE IRQ72, enables it, sets the address, calls `rwip_init` and
  finally initializes the RTOS task. Its code is 120 bytes.
- `bdaddr_init` obtains eight address bytes and passes the last six to
  `co_bdaddr_set`. A port must preserve the vendor's address convention.
- `rw_main_check_task_valid` tests the task handle for non-null.
- `ble_rand` delegates to `bl_rand`.
- `get_stack_usage` scans the stock stack's fill pattern. Its linker-symbol ABI
  is separate from a Rust task's stack usage.
- `platform_reset` changes MIE and handles reset/error paths. It needs deliberate
  adaptation rather than an empty success stub.
- `ble_controller_update_adv_scan_aa` delegates to the link layer;
  `ble_controller_program_ongoing` examines its active-event pointer.
- `rwip_eif_get` in this new on-chip variant asserts and returns null; the old
  standard variant also had a UART interface. This is not an HCI UART transport.
- Library version and debug-level getters/setters are small accessors.

The two objects are not completely identical: the new variant changes reset
and external-interface handling, removes some old wrappers and adds debug-level
accessors. Reusing an entire old object without checking these differences is
not justified.

## What the import audit does not establish

The nine unresolved FreeRTOS references are all in `arch_main.o`. Other objects
still call into the controller's internal scheduler, allocator and ROM code.
Zephyr's stock BL70x integration reserves writable BLE and FreeRTOS ROM state
and configures GP-relative symbols; it also intercepts software IRQ3.
The existing HAL already uses IRQ3 for the software interrupt executor.

Replacing this small wrapper is therefore a promising route to avoiding an
RTOS scheduler, but the remaining ROM call graph, GP/data layout, radio IRQ and
RF initialization still need verification. No radio operation or controller
port is claimed by this disassembly.

## Reproduction

Inside `nix develop`, with a downloaded archive:

```sh
ar p /path/to/controller.a arch_main.o > arch_main.o
size -A arch_main.o
nm -S --size-sort arch_main.o
llvm-objdump -dr arch_main.o > arch_main.asm
llvm-dwarfdump arch_main.o > arch_main.dwarf
```

Use separate working directories for the old and new object. Function selection
by name alone can stop early at local compiler labels in LLVM's disassembler;
disassembling the extracted object or a complete named section avoids that trap.
