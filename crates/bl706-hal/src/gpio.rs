//! BL70x v1 GPIO with one-shot interrupt-driven async input waits.
use crate::{interrupt, pac};
use core::{
    cell::RefCell,
    future::poll_fn,
    task::{Poll, Waker},
};
#[path = "gpio_irq.rs"]
mod irq;
use irq::{Hardware, Trigger};

struct Waiters {
    state: irq::State,
    wakers: [Option<Waker>; 32],
}
static WAITERS: critical_section::Mutex<RefCell<Waiters>> =
    critical_section::Mutex::new(RefCell::new(Waiters {
        state: irq::State::new(),
        wakers: [const { None }; 32],
    }));

struct Registers;
impl Hardware for Registers {
    fn mask(&mut self, pins: u32) {
        pac::GLB.GPIO_INT_MASK1().modify(|r| r.0 |= pins);
        pac::GLB.GPIO_INT2_MASK1().modify(|r| r.0 |= pins);
        fence();
    }
    fn configure(&mut self, pin: u8, primary: u32, secondary: Option<u32>) {
        // Ten logical GPIOs per mode word (not the SF/PSRAM pad index).
        let primary_reg = match pin / 10 {
            0 => pac::GLB.GPIO_INT_MODE_SET1().as_ptr().cast::<u32>(),
            1 => pac::GLB.GPIO_INT_MODE_SET2().as_ptr().cast::<u32>(),
            2 => pac::GLB.GPIO_INT_MODE_SET3().as_ptr().cast::<u32>(),
            _ => pac::GLB.GPIO_INT_MODE_SET4().as_ptr().cast::<u32>(),
        };
        let secondary_reg = match pin / 10 {
            0 => pac::GLB.GPIO_INT2_MODE_SET1().as_ptr().cast::<u32>(),
            1 => pac::GLB.GPIO_INT2_MODE_SET2().as_ptr().cast::<u32>(),
            2 => pac::GLB.GPIO_INT2_MODE_SET3().as_ptr().cast::<u32>(),
            _ => pac::GLB.GPIO_INT2_MODE_SET4().as_ptr().cast::<u32>(),
        };
        for (ptr, mode) in [
            (primary_reg, primary),
            (secondary_reg, secondary.unwrap_or(4)),
        ] {
            // SAFETY: validated pin, aligned RW mode registers; caller holds CS.
            unsafe {
                ptr.write_volatile(irq::mode_word(ptr.read_volatile(), pin, mode));
            }
        }
    }
    fn clear(&mut self, pins: u32) {
        // SDK pulses CLR 1 -> 0. These are not self-clearing W1C bits.
        pac::GLB.GPIO_INT_CLR1().write(|r| r.0 = pins);
        pac::GLB.GPIO_INT2_CLR1().write(|r| r.0 = pins);
        fence();
        pac::GLB.GPIO_INT_CLR1().write(|r| r.0 = 0);
        pac::GLB.GPIO_INT2_CLR1().write(|r| r.0 = 0);
        fence();
    }
    fn unmask(&mut self, pin: u8, secondary: bool) {
        pac::GLB.GPIO_INT_MASK1().modify(|r| r.0 &= !(1 << pin));
        if secondary {
            pac::GLB.GPIO_INT2_MASK1().modify(|r| r.0 &= !(1 << pin));
        }
        fence();
    }
    fn high(&self, pin: u8) -> bool {
        pac::GLB.GPIO_CFGCTL30().read().0 & (1 << pin) != 0
    }
    fn pending(&self) -> u32 {
        (pac::GLB.GPIO_INT_STAT1().read().0 & !pac::GLB.GPIO_INT_MASK1().read().0)
            | (pac::GLB.GPIO_INT2_STAT1().read().0 & !pac::GLB.GPIO_INT2_MASK1().read().0)
    }
}
fn fence() {
    unsafe { core::arch::asm!("fence iorw, iorw") };
}

pub(crate) fn init() {
    let mut hw = Registers;
    hw.mask(u32::MAX);
    hw.clear(u32::MAX);
    // SAFETY: all GPIO sources masked, static state initialized, vector installed.
    unsafe {
        interrupt::enable(interrupt::GPIO);
    }
}

pub(crate) fn on_interrupt() {
    let wakers = critical_section::with(|cs| {
        let mut waiters = WAITERS.borrow(cs).borrow_mut();
        let bits = waiters.state.on_interrupt(&mut Registers);
        core::array::from_fn::<_, 32, _>(|pin| {
            if bits & (1 << pin) != 0 {
                waiters.wakers[pin].take()
            } else {
                None
            }
        })
    });
    // No waker callback may reenter a borrowed RefCell.
    for waker in wakers.into_iter().flatten() {
        waker.wake();
    }
}

struct WaitGuard(u8);
impl Drop for WaitGuard {
    fn drop(&mut self) {
        let waker = critical_section::with(|cs| {
            let mut waiters = WAITERS.borrow(cs).borrow_mut();
            waiters.state.cancel(self.0, &mut Registers);
            waiters.wakers[usize::from(self.0)].take()
        });
        drop(waker);
    }
}
pub struct Pin<const N: u8> {
    _private: (),
}
pub struct Input<const N: u8> {
    _pin: Pin<N>,
}
impl<const N: u8> Drop for Input<N> {
    fn drop(&mut self) {
        // Also cleans up a wait whose future was forgotten instead of dropped.
        drop(WaitGuard(N));
    }
}
pub struct Output<const N: u8> {
    _pin: Pin<N>,
}
#[derive(Clone, Copy)]
pub enum Pull {
    None,
    Up,
    Down,
}

impl<const N: u8> Pin<N> {
    pub(crate) fn configure_output_low(&self) {
        set_output(N, false);
        self.configure(11, Pull::None, false);
        set_oe(N, true);
    }
    pub(crate) const unsafe fn new() -> Self {
        Self { _private: () }
    }
    pub fn into_input(self, pull: Pull) -> Input<N> {
        self.configure(11, pull, true);
        set_oe(N, false);
        Input { _pin: self }
    }
    pub fn into_output(self, high: bool) -> Output<N> {
        set_output(N, high);
        self.configure(11, Pull::None, false);
        set_oe(N, true);
        Output { _pin: self }
    }
    pub(crate) fn configure(&self, function: u8, pull: Pull, input: bool) {
        assert!(N < 32);
        let value = (u32::from(function) << 8)
            | u32::from(input)
            | (u32::from(function != 10) << 1)
            | match pull {
                Pull::None => 0,
                Pull::Up => 1 << 4,
                Pull::Down => 1 << 5,
            };
        critical_section::with(|_| {
            set_oe(N, false);
            // GPIO23..28 may use the dedicated SF/PSRAM pads (SDK realPin).
            let real = if (23..=28).contains(&N)
                && pac::GLB.GPIO_USE_PSRAM__IO().read().0 & (1 << (N - 23)) != 0
            {
                N + 9
            } else {
                N
            };
            // Two 16-bit pin configurations per word. Preserve neighbour.
            let reg = unsafe {
                pac::common::Reg::<u32, pac::common::RW>::from_ptr(
                    (0x4000_0100 + usize::from(real / 2) * 4) as *mut u32,
                )
            };
            let shift = (real % 2) * 16;
            reg.modify(|r| *r = (*r & !(0xffff << shift)) | (value << shift));
            if real != N {
                // SF pads have dedicated electrical controls, but their function
                // mux still lives in the original GPIO23..28 register (SDK).
                let mux = unsafe {
                    pac::common::Reg::<u32, pac::common::RW>::from_ptr(
                        (0x4000_0100 + usize::from(N / 2) * 4) as *mut u32,
                    )
                };
                let shift = (N % 2) * 16 + 8;
                mux.modify(|r| *r = (*r & !(0x1f << shift)) | (u32::from(function) << shift));
            }
            // GPIO9..13 also have an always-on input gate. ROM state must not
            // determine whether a newly configured I2C/GPIO input can be read.
            if (9..=13).contains(&N) {
                pac::HBN.HBN_IRQ_MODE().modify(|r| {
                    let bit = 1 << (N - 9);
                    r.set_reg_aon_pad_ie_smt(
                        (r.reg_aon_pad_ie_smt() & !bit) | if input { bit } else { 0 },
                    );
                });
            }
        });
    }
}
fn set_oe(pin: u8, enabled: bool) {
    critical_section::with(|_| {
        pac::GLB.GPIO_CFGCTL34().modify(|r| {
            r.0 = (r.0 & !(1 << pin)) | (u32::from(enabled) << pin);
        })
    });
}
fn set_output(pin: u8, high: bool) {
    critical_section::with(|_| {
        pac::GLB.GPIO_CFGCTL32().modify(|r| {
            r.0 = (r.0 & !(1 << pin)) | (u32::from(high) << pin);
        })
    });
}
impl<const N: u8> Input<N> {
    pub fn is_high(&self) -> bool {
        pac::GLB.GPIO_CFGCTL30().read().0 & (1 << N) != 0
    }
    pub fn is_low(&self) -> bool {
        !self.is_high()
    }
    async fn wait(&mut self, trigger: Trigger) {
        // Starts on first poll. ISR only accesses static state, never this input.
        let previous = critical_section::with(|cs| {
            let mut waiters = WAITERS.borrow(cs).borrow_mut();
            waiters.state.arm(N, trigger, &mut Registers);
            waiters.wakers[usize::from(N)].take()
        });
        drop(previous);
        let _guard = WaitGuard(N);
        poll_fn(|cx| {
            let mut replacement = Some(cx.waker().clone());
            let ready = critical_section::with(|cs| {
                let mut waiters = WAITERS.borrow(cs).borrow_mut();
                if waiters.state.ready(N) {
                    return true;
                }
                core::mem::swap(&mut waiters.wakers[usize::from(N)], &mut replacement);
                false
            });
            drop(replacement);
            if ready {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
    }
    /// Wait for a low level; returns immediately if already low. Cancellation disarms the pin.
    pub async fn wait_for_low(&mut self) {
        self.wait(Trigger::Low).await;
    }
    /// Wait for a high level; returns immediately if already high.
    pub async fn wait_for_high(&mut self) {
        self.wait(Trigger::High).await;
    }
    /// Wait for a rising edge after arming. Earlier edges are discarded.
    pub async fn wait_for_rising_edge(&mut self) {
        self.wait(Trigger::Rising).await;
    }
    /// Wait for a falling edge after arming. Earlier edges are discarded.
    pub async fn wait_for_falling_edge(&mut self) {
        self.wait(Trigger::Falling).await;
    }
    /// Wait for either edge using both hardware detectors for this GPIO.
    pub async fn wait_for_any_edge(&mut self) {
        self.wait(Trigger::AnyEdge).await;
    }
}
impl<const N: u8> embedded_hal_async::digital::Wait for Input<N> {
    async fn wait_for_high(&mut self) -> Result<(), Self::Error> {
        Input::wait_for_high(self).await;
        Ok(())
    }
    async fn wait_for_low(&mut self) -> Result<(), Self::Error> {
        Input::wait_for_low(self).await;
        Ok(())
    }
    async fn wait_for_rising_edge(&mut self) -> Result<(), Self::Error> {
        Input::wait_for_rising_edge(self).await;
        Ok(())
    }
    async fn wait_for_falling_edge(&mut self) -> Result<(), Self::Error> {
        Input::wait_for_falling_edge(self).await;
        Ok(())
    }
    async fn wait_for_any_edge(&mut self) -> Result<(), Self::Error> {
        Input::wait_for_any_edge(self).await;
        Ok(())
    }
}
impl<const N: u8> Output<N> {
    pub fn set_high(&mut self) {
        set_output(N, true);
    }
    pub fn set_low(&mut self) {
        set_output(N, false);
    }
}
impl<const N: u8> embedded_hal::digital::ErrorType for Input<N> {
    type Error = core::convert::Infallible;
}
impl<const N: u8> embedded_hal::digital::InputPin for Input<N> {
    fn is_high(&mut self) -> Result<bool, Self::Error> {
        Ok(Input::is_high(self))
    }
    fn is_low(&mut self) -> Result<bool, Self::Error> {
        Ok(Input::is_low(self))
    }
}
impl<const N: u8> embedded_hal::digital::ErrorType for Output<N> {
    type Error = core::convert::Infallible;
}
impl<const N: u8> embedded_hal::digital::OutputPin for Output<N> {
    fn set_high(&mut self) -> Result<(), Self::Error> {
        Output::set_high(self);
        Ok(())
    }
    fn set_low(&mut self) -> Result<(), Self::Error> {
        Output::set_low(self);
        Ok(())
    }
}
macro_rules! pins {
    ($($name:ident: $n:literal),* $(,)?) => {
        pub struct Pins { $(pub $name: Pin<$n>),* }
        impl Pins { pub(crate) unsafe fn new() -> Self { Self { $($name: unsafe { Pin::new() }),* } } }
    };
}
pins!(p0:0, p1:1, p2:2, p3:3, p4:4, p5:5, p6:6, p7:7, p8:8, p9:9,
    p10:10, p11:11, p12:12, p13:13, p14:14, p15:15, p16:16, p17:17,
    p18:18, p19:19, p20:20, p21:21, p22:22, p23:23, p24:24, p25:25,
    p26:26, p27:27, p28:28, p29:29, p30:30, p31:31);
