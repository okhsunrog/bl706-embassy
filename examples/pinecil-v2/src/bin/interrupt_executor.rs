//! HIL: software IRQ preemption, timer/I2C wakeups, cross-context mutex and FPU.
#![no_std]
#![no_main]
use bl706_hal::portable_atomic::{AtomicU32, Ordering::Relaxed};
use core::fmt::Write;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use embassy_time::Timer;
use pinecil_v2_bringup::{Logger, banner, board};
use static_cell::StaticCell;

static THREAD: StaticCell<bl706_hal::Executor> = StaticCell::new();
static IRQ: bl706_hal::InterruptExecutor = bl706_hal::InterruptExecutor::new();
static LOCK: Mutex<CriticalSectionRawMutex, ()> = Mutex::new(());
static READS: AtomicU32 = AtomicU32::new(0);

#[embassy_executor::task]
async fn irq_check(mut i2c: bl706_hal::i2c::I2c) {
    use bl706_hal::embedded_hal_async::i2c::I2c;
    loop {
        let guard = LOCK.lock().await;
        // More than the two-word FIFO: progress requires several I2C IRQs
        // while thread mode remains deliberately busy. These registers do not
        // clear events, pop PD packets, or change the PHY's configuration.
        let mut registers = [0; 9];
        i2c.write_read(0x22, &[1], &mut registers).await.unwrap();
        assert_eq!(registers[0], 0x81);
        #[cfg(feature = "fpu")]
        unsafe {
            // Deliberately clobber a caller-saved FP register and FCSR in the
            // IRQ. The busy thread checks its interrupted values afterwards.
            core::arch::asm!(
                "fmv.w.x ft11, {value}", "fscsr {fcsr}",
                value = in(reg) 0x3f80_0000u32,
                fcsr = in(reg) 0x20u32,
                out("ft11") _,
                options(nostack),
            );
        }
        READS.fetch_add(1, Relaxed);
        drop(guard);
        Timer::after_millis(2).await;
    }
}

/// A single asm block keeps the FP probe live across actual IRQ preemption.
/// No Rust call or await can accidentally save it on behalf of the runtime.
fn busy() {
    #[cfg(feature = "fpu")]
    {
        let bits: u32;
        let fcsr: u32;
        unsafe {
            core::arch::asm!(
                "fmv.w.x ft11, {value}", "fscsr {expected_fcsr}",
                "2:", "addi {count}, {count}, -1", "bnez {count}, 2b",
                "fmv.x.w {bits}, ft11", "frcsr {fcsr}", "fscsr zero",
                value = in(reg) 0x4050_0000u32,
                expected_fcsr = in(reg) 0x60u32,
                count = inout(reg) 1_000_000u32 => _,
                bits = lateout(reg) bits,
                fcsr = lateout(reg) fcsr,
                out("ft11") _,
                options(nostack),
            );
        }
        assert_eq!(bits, 0x4050_0000, "IRQ corrupted thread FP register");
        assert_eq!(fcsr, 0x60, "IRQ corrupted thread FCSR");
    }
    #[cfg(not(feature = "fpu"))]
    unsafe {
        core::arch::asm!(
            "2:", "addi {count}, {count}, -1", "bnez {count}, 2b",
            count = inout(reg) 1_000_000u32 => _,
            options(nostack),
        );
    }
}

#[embassy_executor::task]
async fn thread_check(mut log: Logger, mut heater_off: bl706_hal::gpio::Output<21>) {
    banner(&mut log, "interrupt executor");
    let mut round = 0u32;
    loop {
        heater_off.set_low();
        let guard = LOCK.lock().await;
        let before = READS.load(Relaxed);
        busy();
        assert_eq!(READS.load(Relaxed), before, "mutex exclusion failed");
        // Unlock must wake the IRQ executor from thread mode. The following
        // busy loop gives it no cooperation from the thread executor.
        drop(guard);
        busy();
        let reads = READS.load(Relaxed).wrapping_sub(before);
        assert!(
            reads >= 2,
            "software IRQ made no progress during busy thread"
        );
        let _ = writeln!(
            log,
            "round={round} IRQ reads={reads} mutex/timer/I2C OK fpu={}\r",
            cfg!(feature = "fpu")
        );
        round = round.wrapping_add(1);
        Timer::after_millis(100).await;
    }
}

#[unsafe(no_mangle)]
extern "C" fn __bl706_main() -> ! {
    let b = board();
    IRQ.start(b.software_interrupt)
        .spawn(irq_check(b.i2c).unwrap());
    THREAD.init(bl706_hal::Executor::new()).run(|s| {
        s.spawn(thread_check(b.log, b.heater_off).unwrap());
    })
}
