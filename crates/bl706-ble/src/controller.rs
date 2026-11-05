use crate::{Error, Packet, packet::CAPACITY};
use bt_hci::transport::{PacketKind, PacketToController, PacketToHost, Transport};
use core::ffi::c_void;
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, signal::Signal,
};

static HOST: Channel<CriticalSectionRawMutex, usize, 20> = Channel::new();
static RX: Channel<CriticalSectionRawMutex, Packet, 16> = Channel::new();
#[derive(Clone, Copy, Debug, Default)]
pub struct Metrics {
    pub interrupts: u32,
    pub max_interrupt_us: u32,
    pub max_schedule_us: u32,
    pub max_wake_to_worker_us: u32,
}
static METRICS: critical_section::Mutex<core::cell::Cell<Metrics>> =
    critical_section::Mutex::new(core::cell::Cell::new(Metrics {
        interrupts: 0,
        max_interrupt_us: 0,
        max_schedule_us: 0,
        max_wake_to_worker_us: 0,
    }));
static PENDING_SINCE: critical_section::Mutex<core::cell::Cell<Option<u64>>> =
    critical_section::Mutex::new(core::cell::Cell::new(None));
/// Return and clear timing maxima for the previous measurement window.
pub fn take_metrics() -> Metrics {
    critical_section::with(|cs| METRICS.borrow(cs).replace(Metrics::default()))
}
fn update_metrics(f: impl FnOnce(&mut Metrics)) {
    critical_section::with(|cs| {
        let mut m = METRICS.borrow(cs).get();
        f(&mut m);
        METRICS.borrow(cs).set(m);
    });
}
unsafe extern "C" fn radio_interrupt() {
    let start = bl706_hal::time::now_ticks();
    unsafe {
        rwble_isr();
    }
    let elapsed = bl706_hal::time::now_ticks().wrapping_sub(start) as u32;
    update_metrics(|m| {
        m.interrupts = m.interrupts.wrapping_add(1);
        m.max_interrupt_us = m.max_interrupt_us.max(elapsed);
    });
}

static WAKE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

#[repr(C)]
pub(crate) struct TaskMessage {
    kind: u8,
    params: *mut c_void,
}
#[repr(C)]
struct Command {
    opcode: u16,
    params: *const u8,
    len: u8,
}
#[repr(C)]
struct Acl {
    handle: u16,
    flags: u8,
    len: u16,
    data: *const u8,
}
#[repr(C)]
union NativePacket {
    command: core::mem::ManuallyDrop<Command>,
    acl: core::mem::ManuallyDrop<Acl>,
}

// RV32 SDK ABI, independently checked against its original DWARF/header.
const _: () = {
    assert!(core::mem::size_of::<TaskMessage>() == 8);
    assert!(core::mem::offset_of!(TaskMessage, params) == 4);
    assert!(core::mem::size_of::<Command>() == 12);
    assert!(core::mem::offset_of!(Command, params) == 4);
    assert!(core::mem::offset_of!(Command, len) == 8);
    assert!(core::mem::size_of::<Acl>() == 12);
    assert!(core::mem::offset_of!(Acl, len) == 4);
    assert!(core::mem::offset_of!(Acl, data) == 8);
};

unsafe extern "C" {
    fn rwip_init(error: u32);
    fn rwip_schedule();
    fn rwble_isr_clear();
    fn rwble_isr();
    fn co_bdaddr_set(address: *const u8);
    fn hci_send_2_controller(params: *mut c_void);
    fn bt_onchiphci_interface_init(callback: unsafe extern "C" fn(u8, u16, *mut u8, u8)) -> u8;
    fn bt_onchiphci_send(kind: u8, destination: u16, packet: *const NativePacket) -> i8;
    fn bt_onchiphci_handle_rx_acl(params: *mut u8, destination: *mut u8) -> u8;
}

/// The controller has exactly one worker. Run it concurrently with the host.
pub struct Runner {
    _radio: bl706_hal::radio::RadioToken,
}
pub struct NativeTransport {
    _private: (),
}

/// Initialize the official peripheral-only controller without an RTOS.
///
/// # Safety
/// Only supported with the documented BLE linker layout: RF calibration owns
/// OCRAM 0x42024000..0x42028000 and BLE owns the upper 8 KiB exchange memory.
/// No other security-engine user may access TRNG. The frozen root clock must
/// use XTAL32M, and no software interrupt executor may have started yet.
pub unsafe fn initialize(
    radio: bl706_hal::radio::RadioToken,
    _software: &mut bl706_hal::executor::InterruptToken,
) -> Result<(NativeTransport, Runner), Error> {
    crate::platform::prepare();
    rtt_target::rprintln!("BLE: hooks/clocks prepared");
    unsafe {
        bl706_hal::radio::set_handler(Some(radio_interrupt));
        rwble_isr_clear();
        crate::platform::bl_irq_pending_clear(72);
        crate::platform::bl_irq_enable(72);
        co_bdaddr_set(crate::platform::ADDRESS.as_ptr().add(2));
        rwip_init(0);
    }
    rtt_target::rprintln!("BLE: rwip initialized");
    crate::platform::finish_calibration();
    if unsafe { bt_onchiphci_interface_init(receive) } != 0 {
        crate::platform::bl_irq_disable(72);
        unsafe {
            bl706_hal::radio::set_handler(None);
        }
        return Err(Error::Unsupported);
    }
    Ok((NativeTransport { _private: () }, Runner { _radio: radio }))
}
impl Runner {
    pub async fn run(&mut self) -> ! {
        loop {
            let start = bl706_hal::time::now_ticks();
            if let Some(pending) = critical_section::with(|cs| PENDING_SINCE.borrow(cs).take()) {
                update_metrics(|m| {
                    m.max_wake_to_worker_us = m
                        .max_wake_to_worker_us
                        .max(start.wrapping_sub(pending) as u32)
                });
            }
            // The signal retains notifications between scheduling and waiting.
            // HOST messages own vendor heap parameters, consumed by hci_send.
            while let Ok(params) = HOST.try_receive() {
                unsafe {
                    hci_send_2_controller(params as *mut c_void);
                }
            }
            unsafe {
                rwip_schedule();
            }
            let elapsed = bl706_hal::time::now_ticks().wrapping_sub(start) as u32;
            update_metrics(|m| m.max_schedule_us = m.max_schedule_us.max(elapsed));
            crate::platform::refill_entropy();
            WAKE.wait().await;
        }
    }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn rw_main_task_post(message: *const TaskMessage, _timeout: u32) -> bool {
    let message = unsafe { &*message };
    let accepted = match message.kind {
        1 => HOST.try_send(message.params as usize).is_ok(),
        2 => true, // FW scheduling notifications have no owned payload.
        _ => false,
    };
    if accepted {
        WAKE.signal(());
    }
    accepted
}
#[unsafe(no_mangle)]
pub extern "C" fn rw_main_task_post_from_fw() {
    critical_section::with(|cs| {
        if PENDING_SINCE.borrow(cs).get().is_none() {
            PENDING_SINCE
                .borrow(cs)
                .set(Some(bl706_hal::time::now_ticks()));
        }
    });
    WAKE.signal(());
}
#[unsafe(no_mangle)]
extern "C" fn rw_main_task_post_from_isr() {
    rw_main_task_post_from_fw();
}
#[unsafe(no_mangle)]
extern "C" fn rw_main_check_task_valid() -> bool {
    true
}
#[unsafe(no_mangle)]
extern "C" fn rw_main_get_task_priority() -> u8 {
    0
}

unsafe extern "C" fn receive(kind: u8, source: u16, params: *mut u8, len: u8) {
    let packet = if kind == 1 {
        let mut p = Packet::empty(PacketKind::AclData);
        // Extraction consumes the controller's exchange-memory buffer before
        // it frees the message. Never retain the callback's borrowed pointer.
        let n = unsafe { bt_onchiphci_handle_rx_acl(params, p.acl_storage().as_mut_ptr()) };
        p.set_acl_len(usize::from(n))
            .expect("invalid native ACL length");
        p
    } else {
        let data = if len == 0 {
            &[]
        } else {
            assert!(!params.is_null());
            unsafe { core::slice::from_raw_parts(params, usize::from(len)) }
        };
        Packet::event(kind, source, data).expect("invalid native HCI callback")
    };
    // Dropping a command completion or ACL credit would deadlock the host.
    assert!(RX.try_send(packet).is_ok(), "BLE RX queue exhausted");
}
impl embedded_io::ErrorType for NativeTransport {
    type Error = Error;
}
impl Transport for NativeTransport {
    async fn read<'a, P: PacketToHost<'a>>(&self, rx: &'a mut [u8]) -> Result<P, Error> {
        let packet = RX.receive().await;
        P::read_hci(packet.kind, &mut packet.bytes(), rx).map_err(|_| Error::InvalidPacket)
    }
    async fn write<P: PacketToController>(&self, tx: &P) -> Result<(), Error> {
        let mut bytes = [0u8; CAPACITY];
        if tx.size() > bytes.len() {
            return Err(Error::InvalidPacket);
        }
        tx.write_hci(&mut bytes[..])
            .map_err(|_| Error::InvalidPacket)?;
        self.send_raw(P::KIND, &bytes[..tx.size()])
    }
}
impl NativeTransport {
    /// Raw packets omit the H4 indicator, useful for bring-up before the host.
    pub fn send_raw(&self, kind: PacketKind, data: &[u8]) -> Result<(), Error> {
        let (native_kind, destination, packet) = match kind {
            PacketKind::Cmd if data.len() >= 3 && usize::from(data[2]) + 3 == data.len() => {
                let opcode = u16::from_le_bytes([data[0], data[1]]);
                (
                    0,
                    crate::packet::command_destination(opcode, &data[3..])?,
                    NativePacket {
                        command: core::mem::ManuallyDrop::new(Command {
                            opcode,
                            params: data[3..].as_ptr(),
                            len: data[2],
                        }),
                    },
                )
            }
            PacketKind::AclData
                if data.len() >= 4
                    && data.len() <= 255
                    && usize::from(u16::from_le_bytes([data[2], data[3]])) + 4 == data.len() =>
            {
                let header = u16::from_le_bytes([data[0], data[1]]);
                (
                    1,
                    header & 0xfff,
                    NativePacket {
                        acl: core::mem::ManuallyDrop::new(Acl {
                            handle: header & 0xfff,
                            flags: (header >> 12) as u8,
                            len: (data.len() - 4) as u16,
                            data: data[4..].as_ptr(),
                        }),
                    },
                )
            }
            _ => return Err(Error::Unsupported),
        };
        // Verified in bt_onchiphci_send disassembly: command data is unpacked
        // into a KE message and ACL data copied into EM before this returns.
        let status = unsafe { bt_onchiphci_send(native_kind, destination, &packet) };
        if status == 0 {
            Ok(())
        } else {
            Err(Error::Vendor(status))
        }
    }
    pub async fn receive_raw(&self) -> Packet {
        RX.receive().await
    }
}
