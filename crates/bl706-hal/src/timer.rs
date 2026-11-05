//! Two 32-bit peripheral timers. Embassy's clock continues to use core-local mtime.
pub use crate::peripheral_logic::Error;
use crate::{clock::Clocks, interrupt, pac, peripheral_logic::replace_field};
use core::{
    cell::RefCell,
    future::poll_fn,
    task::{Poll, Waker},
};

pub struct TimerToken<const N: u8> {
    _private: (),
}
impl<const N: u8> TimerToken<N> {
    pub(crate) const fn new() -> Self {
        Self { _private: () }
    }
}
#[derive(Clone, Copy, Debug)]
pub enum Compare {
    C0,
    C1,
    C2,
}
#[derive(Clone, Copy, Debug)]
pub enum ClockSource {
    System,
    Clock32K,
    Clock1K,
}
#[derive(Clone, Copy, Debug)]
pub enum Mode {
    FreeRunning,
    Preload { value: u32, trigger: Compare },
}
#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub clock: ClockSource,
    /// Actual divisor, 1..=256 (register stores divisor - 1).
    pub divider: u16,
    pub mode: Mode,
    /// Raw register values. No hidden SDK match-minus-two compensation.
    pub compare: [u32; 3],
}
impl Config {
    fn validate(self) -> Result<(), Error> {
        crate::peripheral_logic::timer_valid(
            self.divider,
            match self.mode {
                Mode::FreeRunning => None,
                Mode::Preload { value, trigger } => Some((value, self.compare[trigger as usize])),
            },
        )
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Events(u8);
impl Events {
    pub const NONE: Self = Self(0);
    pub const ALL: Self = Self(7);
    pub const fn compare(compare: Compare) -> Self {
        Self(1 << compare as u8)
    }
    pub const fn contains(self, compare: Compare) -> bool {
        self.0 & (1 << compare as u8) != 0
    }
    pub const fn bits(self) -> u8 {
        self.0
    }
}
struct WaitState {
    armed: u8,
    complete: u8,
    waker: Option<Waker>,
}
static WAITERS: critical_section::Mutex<RefCell<[WaitState; 2]>> =
    critical_section::Mutex::new(RefCell::new(
        [const {
            WaitState {
                armed: 0,
                complete: 0,
                waker: None,
            }
        }; 2],
    ));

pub struct Timer<const N: u8> {
    _token: TimerToken<N>,
    config: Config,
    clocks: Clocks,
}
fn rw(offset: usize) -> pac::common::Reg<u32, pac::common::RW> {
    // SAFETY: private reviewed RW offsets only, N is checked by Timer::new.
    unsafe { pac::common::Reg::from_ptr(pac::TIMER.as_ptr().cast::<u8>().add(offset).cast()) }
}
fn irq(n: u8) -> u8 {
    interrupt::TIMER0 + n
}
fn status(n: u8) -> u8 {
    if n == 0 {
        pac::TIMER.TMSR2().read().0 as u8 & 7
    } else {
        pac::TIMER.TMSR3().read().0 as u8 & 7
    }
}
fn clear(n: u8, bits: u8) {
    if n == 0 {
        pac::TIMER.TICR2().write(|r| r.0 = u32::from(bits & 7));
    } else {
        pac::TIMER.TICR3().write(|r| r.0 = u32::from(bits & 7));
    }
    crate::io_fence();
}
fn cancel(n: u8, bits: u8) {
    let old = critical_section::with(|cs| {
        rw(0x44 + usize::from(n) * 4).modify(|r| *r &= !u32::from(bits));
        clear(n, bits);
        let mut states = WAITERS.borrow(cs).borrow_mut();
        let state = &mut states[usize::from(n)];
        state.armed &= !bits;
        state.complete &= !bits;
        state.waker.take()
    });
    drop(old);
}
impl<const N: u8> Timer<N> {
    pub fn new(token: TimerToken<N>, clocks: Clocks, config: Config) -> Result<Self, Error> {
        assert!(N < 2);
        config.validate()?;
        critical_section::with(|_| pac::GLB.cgen_cfg1().modify(|r| r.set_timer(true)));
        let mut timer = Self {
            _token: token,
            config,
            clocks,
        };
        timer.configure(config)?;
        Ok(timer)
    }
    pub fn configure(&mut self, config: Config) -> Result<(), Error> {
        config.validate()?;
        self.stop();
        cancel(N, 7);
        interrupt::disable(irq(N));
        critical_section::with(|_| {
            rw(0).modify(|r| *r = replace_field(*r, 3, 2 + 3 * N, config.clock as u32));
            // Hardware calls the two channels TIMER2/3. TCDR's low byte is
            // reserved, not TIMER0; the watchdog owns the top byte.
            pac::TIMER.TCDR().modify(|r| match N {
                0 => r.set_tcdr2((config.divider - 1) as u8),
                _ => r.set_tcdr3((config.divider - 1) as u8),
            });
            let (free, preload, trigger) = match config.mode {
                Mode::FreeRunning => (true, 0, 0),
                Mode::Preload { value, trigger } => (false, value, trigger as u32 + 1),
            };
            rw(0x88).modify(|r| *r = replace_field(*r, 1, N + 1, u32::from(free)));
            rw(0x50 + usize::from(N) * 4).write_value(preload);
            rw(0x5c + usize::from(N) * 4).write_value(trigger);
            rw(0x90 + usize::from(N) * 4).write_value(0); // level IRQs, explicitly acknowledged
            for (i, &value) in config.compare.iter().enumerate() {
                rw(0x10 + usize::from(N) * 12 + i * 4).write_value(value);
            }
            crate::io_fence();
        });
        self.config = config;
        Ok(())
    }
    pub fn start(&mut self) {
        critical_section::with(|_| {
            rw(0x84).modify(|r| *r |= 1 << (N + 1));
            crate::io_fence();
        });
    }
    pub fn stop(&mut self) {
        critical_section::with(|_| {
            rw(0x84).modify(|r| *r &= !(1 << (N + 1)));
            crate::io_fence();
        });
    }
    pub fn is_running(&self) -> bool {
        rw(0x84).read() & (1 << (N + 1)) != 0
    }
    /// Captured count, following SDK request + three reads, not direct TCR access.
    pub fn counter(&mut self) -> u32 {
        let reg = rw(0xa8 + usize::from(N) * 4);
        reg.write_value(1);
        crate::io_fence();
        let _ = reg.read();
        let _ = reg.read();
        reg.read()
    }
    pub fn clock_hz(&self) -> u32 {
        let source = match self.config.clock {
            ClockSource::System => self.clocks.cpu_hz(),
            ClockSource::Clock32K => self.clocks.low_frequency_hz(),
            ClockSource::Clock1K => self.clocks.low_frequency_hz() / 32,
        };
        source / u32::from(self.config.divider)
    }
    /// Updating a running comparator can miss a match; the owner must choose a future value.
    pub fn set_compare(&mut self, compare: Compare, raw: u32) -> Result<(), Error> {
        let mut config = self.config;
        config.compare[compare as usize] = raw;
        config.validate()?;
        rw(0x10 + usize::from(N) * 12 + compare as usize * 4).write_value(raw);
        crate::io_fence();
        self.config = config;
        Ok(())
    }
    /// Set interrupt enables for the static ISR driver, or collect one-shot events via take_events.
    pub fn listen(&mut self, events: Events) {
        critical_section::with(|_| {
            rw(0x44 + usize::from(N) * 4).write_value(u32::from(events.0));
            unsafe {
                interrupt::enable(irq(N));
            }
        });
    }
    pub fn take_events(&mut self) -> Events {
        critical_section::with(|cs| {
            let mut states = WAITERS.borrow(cs).borrow_mut();
            let bits = states[N as usize].complete;
            states[N as usize].complete = 0;
            Events(bits)
        })
    }
    /// Wait for the next match of an already configured, running counter.
    /// Cancellation masks/clears this comparator but does not stop the counter.
    pub async fn wait_for_compare(&mut self, compare: Compare) -> Result<(), Error> {
        if !self.is_running() {
            return Err(Error::Disabled);
        }
        let bit = 1 << compare as u8;
        cancel(N, bit);
        critical_section::with(|cs| {
            WAITERS.borrow(cs).borrow_mut()[N as usize].armed = bit;
            rw(0x44 + usize::from(N) * 4).modify(|r| *r |= u32::from(bit));
            unsafe {
                interrupt::enable(irq(N));
            }
        });
        struct Guard(u8, u8);
        impl Drop for Guard {
            fn drop(&mut self) {
                cancel(self.0, self.1);
            }
        }
        let _guard = Guard(N, bit);
        poll_fn(|cx| {
            let mut waker = Some(cx.waker().clone());
            let ready = critical_section::with(|cs| {
                let mut states = WAITERS.borrow(cs).borrow_mut();
                let s = &mut states[N as usize];
                if s.complete & bit != 0 {
                    return true;
                }
                core::mem::swap(&mut s.waker, &mut waker);
                false
            });
            drop(waker);
            if ready {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
        Ok(())
    }
}
impl<const N: u8> Drop for Timer<N> {
    fn drop(&mut self) {
        self.stop();
        cancel(N, 7);
        interrupt::disable(irq(N));
    }
}

/// Own ADC/PWM/etc in this object to do bounded work directly in the timer ISR.
/// Calls are non-nested; do not block, log, or wait for another interrupt here.
pub trait Handler<const N: u8>: Send + 'static {
    fn on_interrupt(&mut self, timer: &mut Timer<N>, events: Events);
}
pub struct InterruptDriver<const N: u8, H: Handler<N>> {
    inner: critical_section::Mutex<RefCell<DriverInner<N, H>>>,
}
struct DriverInner<const N: u8, H: Handler<N>> {
    timer: Timer<N>,
    handler: H,
}
trait Registered: Sync {
    fn dispatch(&self);
}
static HANDLERS: critical_section::Mutex<RefCell<[Option<&'static dyn Registered>; 2]>> =
    critical_section::Mutex::new(RefCell::new([None, None]));
impl<const N: u8, H: Handler<N>> InterruptDriver<N, H> {
    pub fn new(mut timer: Timer<N>, handler: H) -> Self {
        timer.stop();
        cancel(N, 7);
        Self {
            inner: critical_section::Mutex::new(RefCell::new(DriverInner { timer, handler })),
        }
    }
    /// Register static storage and start the timer. Initialize with StaticCell.
    pub fn register(&'static self, events: Events) {
        critical_section::with(|cs| {
            let mut handlers = HANDLERS.borrow(cs).borrow_mut();
            assert!(handlers[N as usize].is_none());
            clear(N, 7);
            let mut inner = self.inner.borrow(cs).borrow_mut();
            inner.timer.listen(events);
            inner.timer.start();
            handlers[N as usize] = Some(self);
        });
    }
    /// Access the ISR-owned peripherals without aliasing its handler. The
    /// closure runs with interrupts masked: keep it bounded, never wait/log,
    /// and do not reenter this driver from the closure or the handler.
    pub fn with<R>(&self, f: impl FnOnce(&mut Timer<N>, &mut H) -> R) -> R {
        critical_section::with(|cs| {
            let mut inner = self.inner.borrow(cs).borrow_mut();
            let DriverInner { timer, handler } = &mut *inner;
            f(timer, handler)
        })
    }
}
impl<const N: u8, H: Handler<N>> Registered for InterruptDriver<N, H> {
    fn dispatch(&self) {
        self.with(|timer, handler| {
            let bits = status(N) & rw(0x44 + usize::from(N) * 4).read() as u8;
            if bits != 0 {
                clear(N, bits);
                handler.on_interrupt(timer, Events(bits));
            }
        });
    }
}
pub(crate) fn on_interrupt(n: u8) {
    let handler = critical_section::with(|cs| HANDLERS.borrow(cs).borrow()[n as usize]);
    if let Some(handler) = handler {
        handler.dispatch();
        return;
    }
    let waker = critical_section::with(|cs| {
        let bits = status(n) & rw(0x44 + usize::from(n) * 4).read() as u8;
        rw(0x44 + usize::from(n) * 4).modify(|r| *r &= !u32::from(bits));
        clear(n, bits);
        let mut states = WAITERS.borrow(cs).borrow_mut();
        let s = &mut states[n as usize];
        s.complete |= bits;
        if s.armed & bits != 0 {
            s.waker.take()
        } else {
            None
        }
    });
    if let Some(waker) = waker {
        waker.wake();
    }
}
