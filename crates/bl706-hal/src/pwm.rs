//! Five independent PWM counters, BCLK source, normal polarity, abrupt stop.
pub use crate::peripheral_logic::Error;
use crate::{clock::Clocks, gpio, interrupt, pac, peripheral_logic::pwm_valid};
use core::{
    cell::RefCell,
    future::poll_fn,
    task::{Poll, Waker},
};
pub struct PwmToken<const N: u8> {
    _private: (),
}
impl<const N: u8> PwmToken<N> {
    pub(crate) const fn new() -> Self {
        Self { _private: () }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub divider: u16,
    pub period: u16,
}
mod sealed {
    pub trait Sealed {}
}
/// A reviewed PWM routing. GPIO function 8 selects channel pin % 5.
pub trait Output<const N: u8>: sealed::Sealed + Send {
    #[doc(hidden)]
    fn connect(&self);
    #[doc(hidden)]
    fn disconnect(&self);
}
/// Exercise the counter and IRQ without connecting any pad.
pub struct Unrouted;
impl sealed::Sealed for Unrouted {}
impl<const N: u8> Output<N> for Unrouted {
    fn connect(&self) {}
    fn disconnect(&self) {}
}
macro_rules! routes {
    ($($pin:literal => $channel:literal),* $(,)?) => { $(
        impl sealed::Sealed for gpio::Pin<$pin> {}
        impl Output<$channel> for gpio::Pin<$pin> {
            fn connect(&self) { self.configure(8,gpio::Pull::None,false); }
            fn disconnect(&self) { self.configure_output_low(); }
        }
    )* };
}
routes!(0=>0,1=>1,2=>2,3=>3,4=>4,5=>0,6=>1,7=>2,8=>3,9=>4,10=>0,11=>1,
    12=>2,13=>3,14=>4,15=>0,16=>1,17=>2,18=>3,19=>4,20=>0,21=>1,22=>2,
    23=>3,24=>4,25=>0,26=>1,27=>2,28=>3,29=>4,30=>0,31=>1);

struct Waiter {
    armed: bool,
    complete: Option<Result<(), Error>>,
    waker: Option<Waker>,
}
static WAITERS: critical_section::Mutex<RefCell<[Waiter; 5]>> =
    critical_section::Mutex::new(RefCell::new(
        [const {
            Waiter {
                armed: false,
                complete: None,
                waker: None,
            }
        }; 5],
    ));
fn reg(n: u8, offset: usize) -> pac::common::Reg<u32, pac::common::RW> {
    // SAFETY: private per-channel RW offsets, validated N.
    unsafe {
        pac::common::Reg::from_ptr(
            pac::PWM
                .as_ptr()
                .cast::<u8>()
                .add(0x20 + usize::from(n) * 0x20 + offset)
                .cast(),
        )
    }
}
fn clear(bits: u8) -> bool {
    critical_section::with(|_| {
        pac::PWM
            .pwm_int_config()
            .write(|r| r.set_pwm_int_clear(bits));
        crate::io_fence();
        // SDK requires observing status cleared before returning the clear line low.
        // Bound the number of APB reads even if the clock domain is malfunctioning.
        let mut cleared = false;
        for _ in 0..32 {
            if pac::PWM.pwm_int_config().read().pwm_interrupt_sts() & bits == 0 {
                cleared = true;
                break;
            }
        }
        pac::PWM.pwm_int_config().write(|r| r.set_pwm_int_clear(0));
        crate::io_fence();
        cleared
    })
}
fn cancel(n: u8) {
    let waker = critical_section::with(|cs| {
        reg(n, 0x14).modify(|r| *r &= !(1 << 16));
        clear(1 << n);
        let mut states = WAITERS.borrow(cs).borrow_mut();
        let s = &mut states[n as usize];
        s.armed = false;
        s.complete = None;
        s.waker.take()
    });
    drop(waker);
}
pub struct Pwm<const N: u8, P: Output<N>> {
    _token: PwmToken<N>,
    pin: P,
    config: Config,
    duty: u16,
    enabled: bool,
    clocks: Clocks,
}
impl<const N: u8, P: Output<N>> Pwm<N, P> {
    /// Starts forced low and disabled, including before the pad is routed.
    pub fn new(token: PwmToken<N>, pin: P, clocks: Clocks, config: Config) -> Result<Self, Error> {
        assert!(N < 5);
        pwm_valid(config.divider, config.period, 0)?;
        pin.disconnect();
        critical_section::with(|_| pac::GLB.cgen_cfg1().modify(|r| r.set_pwm(true)));
        reg(N, 0x10).write_value(1 | (1 << 5) | (1 << 6));
        let mut pwm = Self {
            _token: token,
            pin,
            config,
            duty: 0,
            enabled: false,
            clocks,
        };
        pwm.configure(config)?;
        pwm.pin.connect();
        Ok(pwm)
    }
    pub fn configure(&mut self, config: Config) -> Result<(), Error> {
        pwm_valid(config.divider, config.period, 0)?;
        self.disable();
        cancel(N);
        let deadline = crate::time::now_ticks() + 1_000;
        let mut polls = 100_000u32;
        while reg(N, 0x10).read() & (1 << 7) == 0 {
            polls -= 1;
            if polls == 0 || crate::time::now_ticks() >= deadline {
                return Err(Error::Timeout);
            }
        }
        // BCLK=1, normal polarity, abrupt stop, software-low, stopped.
        reg(N, 0x10).write_value(1 | (1 << 5) | (1 << 6));
        reg(N, 0).write_value(u32::from(config.divider));
        reg(N, 4).write_value(0);
        reg(N, 8).write_value(0);
        reg(N, 0xc).write_value(u32::from(config.period));
        reg(N, 0x14).write_value(0);
        crate::io_fence();
        self.config = config;
        self.duty = 0;
        Ok(())
    }
    pub fn frequency_hz(&self) -> u32 {
        self.clocks.bus_hz() / u32::from(self.config.divider) / u32::from(self.config.period)
    }
    pub fn max_duty_cycle(&self) -> u16 {
        self.config.period
    }
    pub fn duty_cycle(&self) -> u16 {
        self.duty
    }
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
    pub fn set_duty_cycle(&mut self, duty: u16) -> Result<(), Error> {
        pwm_valid(self.config.divider, self.config.period, duty)?;
        reg(N, 8).write_value(u32::from(duty));
        self.duty = duty;
        if self.enabled {
            self.apply_output();
        }
        Ok(())
    }
    fn apply_output(&self) {
        reg(N, 0x10).modify(|r| {
            *r &= !((1 << 4) | (1 << 5) | (1 << 6));
            // Explicit SW force gives exact 0/100%; intermediate duty uses comparator.
            if self.duty == 0 {
                *r |= 1 << 5;
            } else if self.duty == self.config.period {
                *r |= (1 << 4) | (1 << 5);
            }
        });
        crate::io_fence();
    }
    pub fn enable(&mut self) {
        self.apply_output();
        self.enabled = true;
    }
    /// Force the normal-polarity output low before stopping the counter. No stop polling.
    pub fn disable(&mut self) {
        reg(N, 0x10).modify(|r| *r = (*r & !(1 << 4)) | (1 << 5));
        crate::io_fence();
        reg(N, 0x10).modify(|r| *r |= 1 << 6);
        crate::io_fence();
        self.enabled = false;
    }
    /// One-shot period-count IRQ. Cancellation disarms the IRQ, leaving output unchanged.
    pub async fn wait_periods(&mut self, periods: u16) -> Result<(), Error> {
        if !self.enabled {
            return Err(Error::Disabled);
        }
        if periods == 0 {
            return Err(Error::InvalidConfig);
        }
        cancel(N);
        if !clear(1 << N) {
            return Err(Error::Timeout);
        }
        critical_section::with(|cs| {
            WAITERS.borrow(cs).borrow_mut()[N as usize].armed = true;
            reg(N, 0x14).write_value(u32::from(periods) | (1 << 16));
            unsafe {
                interrupt::enable(interrupt::PWM);
            }
        });
        struct Guard(u8);
        impl Drop for Guard {
            fn drop(&mut self) {
                cancel(self.0);
            }
        }
        let _guard = Guard(N);
        poll_fn(|cx| {
            let mut waker = Some(cx.waker().clone());
            let result = critical_section::with(|cs| {
                let mut states = WAITERS.borrow(cs).borrow_mut();
                let s = &mut states[N as usize];
                if let Some(result) = s.complete {
                    return Some(result);
                }
                core::mem::swap(&mut s.waker, &mut waker);
                None
            });
            drop(waker);
            match result {
                Some(result) => Poll::Ready(result),
                None => Poll::Pending,
            }
        })
        .await
    }
}
impl<const N: u8, P: Output<N>> Drop for Pwm<N, P> {
    fn drop(&mut self) {
        self.disable();
        cancel(N);
        self.pin.disconnect();
    }
}
impl<const N: u8, P: Output<N>> embedded_hal::pwm::ErrorType for Pwm<N, P> {
    type Error = Error;
}
impl embedded_hal::pwm::Error for Error {
    fn kind(&self) -> embedded_hal::pwm::ErrorKind {
        embedded_hal::pwm::ErrorKind::Other
    }
}
impl<const N: u8, P: Output<N>> embedded_hal::pwm::SetDutyCycle for Pwm<N, P> {
    fn max_duty_cycle(&self) -> u16 {
        self.max_duty_cycle()
    }
    fn set_duty_cycle(&mut self, duty: u16) -> Result<(), Error> {
        self.set_duty_cycle(duty)
    }
}
pub(crate) fn on_interrupt() {
    let wakers = critical_section::with(|cs| {
        let bits = pac::PWM.pwm_int_config().read().pwm_interrupt_sts();
        for n in 0..5 {
            if bits & (1 << n) != 0 {
                reg(n, 0x14).modify(|r| *r &= !(1 << 16));
            }
        }
        let cleared = clear(bits);
        let mut states = WAITERS.borrow(cs).borrow_mut();
        core::array::from_fn::<_, 5, _>(|n| {
            if bits & (1 << n) == 0 {
                return None;
            }
            reg(n as u8, 0x14).modify(|r| *r &= !(1 << 16));
            let s = &mut states[n];
            if !s.armed {
                return None;
            }
            s.armed = false;
            s.complete = Some(if cleared { Ok(()) } else { Err(Error::Timeout) });
            s.waker.take()
        })
    });
    for waker in wakers.into_iter().flatten() {
        waker.wake();
    }
}
