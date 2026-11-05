//! ABI boundary replacing arch_main.o and the SDK's small platform interface.
//! MMIO here executes on the target, never through JTAG system-bus reads.
#![allow(non_snake_case)]
use bl706_hal::{interrupt, radio, time};
use core::cell::{Cell, RefCell};
use core::{
    ffi::{CStr, c_char, c_int, c_void},
    ptr::{read_volatile, write_volatile},
};
use critical_section::Mutex;

// A locally administered bootstrap address, replaced by the host's random
// address before advertising. This is deliberately not called a factory MAC.
pub(crate) static ADDRESS: [u8; 8] = [0, 0, 0x06, 0x07, 0x0b, 0x00, 0x00, 0xc2];
static CALIBRATING: Mutex<Cell<bool>> = Mutex::new(Cell::new(false));

unsafe extern "C" {
    fn memcpy(dst: *mut c_void, src: *const c_void, n: usize) -> *mut c_void;
    fn memset(dst: *mut c_void, c: c_int, n: usize) -> *mut c_void;
    fn memcmp(a: *const c_void, b: *const c_void, n: usize) -> c_int;
}
#[unsafe(no_mangle)]
static ble_memcpy_ptr: unsafe extern "C" fn(*mut c_void, *const c_void, usize) -> *mut c_void =
    memcpy;
#[unsafe(no_mangle)]
static ble_memset_ptr: unsafe extern "C" fn(*mut c_void, c_int, usize) -> *mut c_void = memset;
#[unsafe(no_mangle)]
static ble_memcmp_ptr: unsafe extern "C" fn(*const c_void, *const c_void, usize) -> c_int = memcmp;
#[unsafe(no_mangle)]
static ble_post_task_ptr: extern "C" fn() = crate::controller::rw_main_task_post_from_fw;
#[unsafe(no_mangle)]
static _rom_patch_hook: unsafe extern "C" fn(u32, ...) -> u32 = rom_patch;
unsafe extern "C" fn rom_patch(_id: u32, mut _args: ...) -> u32 {
    0
}
#[unsafe(no_mangle)]
extern "C" fn platform_reset(reason: u32) -> ! {
    panic!("BLE platform reset: {reason:#x}")
}

pub(crate) fn prepare() {
    critical_section::with(|cs| CALIBRATING.borrow(cs).set(true));
    let glb = bl706_hal::pac::GLB;
    // GLB_EM_8KB is 0x3, not a size in KiB (SDK bl702_glb.h).
    glb.seam_misc().modify(|r| r.set_em_sel(3));
    glb.clk_cfg1().modify(|r| r.set_ble_en(true));
    glb.cgen_cfg2().modify(|r| r.set_cgen_s3(true));
    // Exercise entropy before the blob starts; errors must not become a
    // predictable pseudo-random fallback.
    refill_entropy();
}
pub(crate) fn finish_calibration() {
    assert!(
        radio::boot_software_handler().is_none(),
        "RF failed to restore IRQ3"
    );
    critical_section::with(|cs| CALIBRATING.borrow(cs).set(false));
}
fn checked_irq(n: u32) -> u8 {
    assert!(
        n == 72 || (n == 3 && critical_section::with(|cs| CALIBRATING.borrow(cs).get())),
        "unexpected BLE IRQ {n}"
    );
    n as u8
}
#[unsafe(no_mangle)]
pub(crate) extern "C" fn bl_irq_enable(n: u32) {
    unsafe {
        interrupt::enable(checked_irq(n));
    }
}
#[unsafe(no_mangle)]
pub(crate) extern "C" fn bl_irq_disable(n: u32) {
    unsafe {
        write_volatile((0x02800400 + u32::from(checked_irq(n))) as *mut u8, 0);
    }
}
#[unsafe(no_mangle)]
pub(crate) extern "C" fn bl_irq_pending_clear(n: u32) {
    unsafe {
        write_volatile((0x02800000 + u32::from(checked_irq(n))) as *mut u8, 0);
    }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn bl_irq_register(n: u32, callback: Option<radio::Handler>) {
    match checked_irq(n) {
        3 => unsafe {
            radio::set_boot_software_handler(callback);
        },
        72 => unsafe {
            radio::set_handler(callback);
        },
        _ => unreachable!(),
    }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn bl_irq_unregister(n: u32, _callback: Option<radio::Handler>) {
    unsafe {
        bl_irq_register(n, None);
    }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn bl_irq_handler_get(n: u32, out: *mut Option<radio::Handler>) {
    unsafe {
        out.write(if checked_irq(n) == 3 {
            radio::boot_software_handler()
        } else {
            radio::handler()
        });
    }
}
#[unsafe(no_mangle)]
extern "C" fn bl_irq_save() -> usize {
    let old: usize;
    unsafe {
        core::arch::asm!("csrrci {}, mstatus, 8", out(reg) old);
    }
    old
}
#[unsafe(no_mangle)]
unsafe extern "C" fn bl_irq_restore(flags: usize) {
    unsafe {
        core::arch::asm!("csrw mstatus, {}", in(reg) flags);
    }
}
#[unsafe(no_mangle)]
extern "C" fn BL702_Delay_US(us: u32) {
    let start = time::now_ticks();
    while time::now_ticks().wrapping_sub(start) < u64::from(us) {
        core::hint::spin_loop();
    }
}
#[unsafe(no_mangle)]
extern "C" fn BL702_Delay_MS(ms: u32) {
    for _ in 0..ms {
        BL702_Delay_US(1000);
    }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn arch_memcpy4(dst: *mut u32, src: *const u32, words: u32) -> *mut u32 {
    unsafe {
        core::ptr::copy_nonoverlapping(src, dst, words as usize);
    }
    dst
}
#[unsafe(no_mangle)]
unsafe extern "C" fn bl_wireless_mac_addr_get(dst: *mut u8) -> c_int {
    unsafe {
        core::ptr::copy_nonoverlapping(ADDRESS.as_ptr(), dst, 8);
    }
    0
}
#[unsafe(no_mangle)]
extern "C" fn abs(x: c_int) -> c_int {
    x.wrapping_abs()
}

// Fresh TRNG words are prepared in thread context. A radio interrupt must
// never poll a hardware RNG, or race another RNG caller. Exhaustion is fatal
// rather than silently falling back to predictable values.
static ENTROPY: Mutex<RefCell<heapless::Vec<u32, 128>>> =
    Mutex::new(RefCell::new(heapless::Vec::new()));
#[unsafe(no_mangle)]
extern "C" fn bl_rand() -> c_int {
    critical_section::with(|cs| {
        ENTROPY
            .borrow(cs)
            .borrow_mut()
            .pop()
            .expect("BLE entropy exhausted")
    }) as c_int
}
pub(crate) fn refill_entropy() {
    while critical_section::with(|cs| ENTROPY.borrow(cs).borrow().len()) <= 120 {
        let block = entropy_block();
        critical_section::with(|cs| {
            ENTROPY
                .borrow(cs)
                .borrow_mut()
                .extend_from_slice(&block)
                .unwrap()
        });
    }
}
// SDK Sec_Eng_Trng_Enable/Read sequence, with IRQs enabled and finite timeout.
fn entropy_block() -> [u32; 8] {
    const CTRL: *mut u32 = 0x40004200 as *mut u32;
    const ENABLE: u32 = 1 << 2;
    const CLEAR_INT: u32 = 1 << 9;
    const MASK_INT: u32 = 1 << 11;
    const TRIGGER: u32 = 1 << 1;
    const CLEAR_DATA: u32 = 1 << 3;
    fn ready() {
        let start = time::now_ticks();
        loop {
            let status = unsafe { read_volatile(CTRL) };
            assert!(status & (1 << 4) == 0, "TRNG health failure");
            if status & 1 == 0 {
                break;
            }
            assert!(
                time::now_ticks().wrapping_sub(start) < 100_000,
                "TRNG timeout"
            );
        }
    }
    unsafe {
        let base = (read_volatile(CTRL) | ENABLE | MASK_INT | CLEAR_INT) & !(TRIGGER | CLEAR_DATA);
        write_volatile(CTRL, base);
        core::arch::asm!("nop", "nop", "nop", "nop");
        ready();
        write_volatile(CTRL, base | TRIGGER);
        core::arch::asm!("nop", "nop", "nop", "nop");
        ready();
        let value = core::array::from_fn(|i| read_volatile((0x40004208 + i * 4) as *const u32));
        write_volatile(CTRL, base | CLEAR_DATA);
        write_volatile(CTRL, base & !ENABLE);
        value
    }
}

// Native Rust variadics on the pinned toolchain. Unsupported conversions stop
// parsing rather than extracting the next argument with an incorrect ABI type.
#[unsafe(no_mangle)]
unsafe extern "C" fn printf(format: *const c_char, mut args: ...) -> c_int {
    use core::fmt::Write;
    let format = unsafe { CStr::from_ptr(format) }.to_bytes();
    let mut text = heapless::String::<512>::new();
    let mut i = 0;
    while i < format.len() {
        if format[i] != b'%' {
            let _ = text.push(format[i] as char);
            i += 1;
            continue;
        }
        i += 1;
        if format.get(i) == Some(&b'%') {
            let _ = text.push('%');
            i += 1;
            continue;
        }
        while format
            .get(i)
            .is_some_and(|b| b"-+ #0.0123456789".contains(b))
        {
            i += 1;
        }
        let mut longs = 0;
        while format.get(i) == Some(&b'l') {
            longs += 1;
            i += 1;
        }
        // RV32 size_t and ptrdiff_t use the integer argument register ABI.
        if matches!(format.get(i), Some(b'z' | b't' | b'h')) {
            i += 1;
            if format.get(i) == Some(&b'h') {
                i += 1;
            }
        }
        let Some(&conversion) = format.get(i) else {
            break;
        };
        i += 1;
        unsafe {
            match conversion {
                b's' => {
                    let p = args.next_arg::<*const c_char>();
                    if p.is_null() {
                        let _ = text.push_str("(null)");
                    } else {
                        let s = CStr::from_ptr(p).to_bytes();
                        for &c in s.iter().take(256) {
                            let _ = text.push(c as char);
                        }
                    }
                }
                b'p' => {
                    let p = args.next_arg::<*const c_void>();
                    let _ = write!(text, "{p:p}");
                }
                b'd' | b'i' => {
                    let value = if longs >= 2 {
                        args.next_arg::<i64>()
                    } else {
                        i64::from(args.next_arg::<c_int>())
                    };
                    let _ = write!(text, "{value}");
                }
                b'x' | b'X' | b'u' | b'o' => {
                    let value = if longs >= 2 {
                        args.next_arg::<u64>()
                    } else {
                        u64::from(args.next_arg::<u32>())
                    };
                    match conversion {
                        b'x' | b'X' => {
                            let _ = write!(text, "{value:x}");
                        }
                        b'o' => {
                            let _ = write!(text, "{value:o}");
                        }
                        _ => {
                            let _ = write!(text, "{value}");
                        }
                    }
                }
                b'c' => {
                    let _ = text.push(args.next_arg::<c_int>() as u8 as char);
                }
                b'f' | b'g' | b'e' => {
                    let value = args.next_arg::<f64>();
                    let _ = write!(text, "{value}");
                }
                _ => {
                    let _ = text.push_str("[unsupported printf]");
                    break;
                }
            }
        }
    }
    rtt_target::rprint!("{}", text.as_str());
    text.len() as c_int
}
#[unsafe(no_mangle)]
unsafe extern "C" fn puts(s: *const c_char) -> c_int {
    unsafe { printf(c"%s\n".as_ptr(), s) }
}

static DEBUG_LEVEL: Mutex<Cell<u32>> = Mutex::new(Cell::new(0));
#[unsafe(no_mangle)]
extern "C" fn ble_controller_get_debug_level() -> u32 {
    critical_section::with(|cs| DEBUG_LEVEL.borrow(cs).get())
}
#[unsafe(no_mangle)]
extern "C" fn ble_controller_set_debug_level(level: u32) {
    critical_section::with(|cs| DEBUG_LEVEL.borrow(cs).set(level));
}
