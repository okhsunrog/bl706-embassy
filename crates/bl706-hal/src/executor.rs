//! Embassy executors for the single-hart BL706 runtime.
//!
//! Do not enable an Embassy `platform-*` or `executor-*` feature: this module
//! supplies the platform's `__pender` and both executor implementations.
use core::{
    cell::{Cell, UnsafeCell},
    marker::PhantomData,
    mem::MaybeUninit,
};
use critical_section::Mutex;
use embassy_executor::{SendSpawner, Spawner, raw};

use crate::interrupt;

static INTERRUPT_EXECUTOR: Mutex<Cell<Option<&'static InterruptExecutor>>> =
    Mutex::new(Cell::new(None));

/// Ownership of the machine software interrupt, obtained from [`crate::init`].
pub struct InterruptToken {
    _private: (),
}
impl InterruptToken {
    pub(crate) const fn new() -> Self {
        Self { _private: () }
    }
}

/// Cooperative thread executor. Wakeups are observed by the polling loop.
pub struct Executor {
    inner: raw::Executor,
    _not_send: PhantomData<*mut ()>,
}
impl Default for Executor {
    fn default() -> Self {
        Self::new()
    }
}
impl Executor {
    pub fn new() -> Self {
        Self {
            inner: raw::Executor::new(core::ptr::null_mut()),
            _not_send: PhantomData,
        }
    }

    /// Spawn initial tasks, then poll forever in the calling thread.
    pub fn run(&'static mut self, init: impl FnOnce(Spawner)) -> ! {
        init(self.inner.spawner());
        loop {
            // SAFETY: run owns this executor forever; the interrupt handler
            // only polls the separate InterruptExecutor.
            unsafe { self.inner.poll() };
            core::hint::spin_loop();
        }
    }
}

/// Executor polled by machine software interrupt 3, preempting thread tasks.
///
/// Peripheral interrupts preempt this executor at a higher CLIC level. Each task
/// poll must still be short and must never block waiting for another task.
/// Use async I/O and a mutex
/// with CriticalSectionRawMutex when sharing peripherals with thread tasks.
/// The IRQ uses the interrupted thread's stack and saves the full FPU context.
pub struct InterruptExecutor {
    inner: UnsafeCell<MaybeUninit<raw::Executor>>,
}
// SAFETY: initialization is protected by a critical section and consumes the
// unique IRQ token. Only the software ISR polls the raw executor; its CLIC level
// prevents software-IRQ reentry, including while a peripheral ISR wakes it.
// other contexts receive only SendSpawner, which enforces Send task futures.
unsafe impl Sync for InterruptExecutor {}

impl Default for InterruptExecutor {
    fn default() -> Self {
        Self::new()
    }
}
impl InterruptExecutor {
    pub const fn new() -> Self {
        Self {
            inner: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    /// Install this executor after HAL initialization and consume IRQ ownership.
    /// Global interrupts must be enabled for spawned tasks to make progress.
    pub fn start(&'static self, _token: InterruptToken) -> SendSpawner {
        critical_section::with(|cs| {
            assert!(
                INTERRUPT_EXECUTOR.borrow(cs).get().is_none(),
                "machine software executor already started"
            );
            // Non-null context selects software IRQ wakeups in __pender. It
            // is an identifier, not a pointer that will ever be dereferenced.
            let context = core::ptr::without_provenance_mut(1);
            // SAFETY: unique token, no installed handler and IRQ masked since
            // init. Publish only after the executor has been fully initialized.
            let executor = unsafe { (&mut *self.inner.get()).write(raw::Executor::new(context)) };
            INTERRUPT_EXECUTOR.borrow(cs).set(Some(self));
            interrupt::clear_software();
            // SAFETY: the initialized handler state is now published.
            unsafe { interrupt::enable(interrupt::MSOFT) };
            executor.spawner().make_send()
        })
    }
}

#[unsafe(export_name = "__pender")]
fn pender(context: *mut ()) {
    if !context.is_null() {
        // May run in thread mode, another ISR, or during the executor's poll.
        // Pend only: calling poll here would permit reentrant executor access.
        interrupt::pend_software();
    }
}

pub(crate) fn on_interrupt() {
    // Clear before polling: a wake during poll must request another IRQ, rather
    // than being lost when the handler returns. MMIO writes are fenced.
    interrupt::clear_software();
    let executor = critical_section::with(|cs| INTERRUPT_EXECUTOR.borrow(cs).get())
        .expect("software interrupt without executor");
    // SAFETY: the IRQ is unmasked only after start publishes initialized state.
    // A second software IRQ has the same CLIC level and cannot preempt this
    // handler. Higher-level peripheral ISRs can only wake it, never poll it.
    unsafe { (&*executor.inner.get()).assume_init_ref().poll() };
}
