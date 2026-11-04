//! Reset-mode watchdog. Drop deliberately does not stop an armed watchdog.
use crate::{
    clock::{Clocks, Error},
    pac,
};
use core::time::Duration;
pub struct WatchdogToken {
    _private: (),
}
impl WatchdogToken {
    pub(crate) const fn new() -> Self {
        Self { _private: () }
    }
}
#[derive(Clone, Copy, Debug)]
pub enum ClockSource {
    Rc32k,
    System,
}
#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub clock: ClockSource,
    pub divider: u16,
    pub timeout: Duration,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            clock: ClockSource::Rc32k,
            divider: 32,
            timeout: Duration::from_secs(2),
        }
    }
}
pub(crate) fn unlock() {
    pac::TIMER.WFAR().write(|r| r.0 = 0xbaba);
    pac::TIMER.WSAR().write(|r| r.0 = 0xeb10);
    crate::io_fence();
}
pub struct Watchdog {
    _token: WatchdogToken,
    match_ticks: u16,
    source_hz: u32,
    divider: u16,
}
impl Watchdog {
    /// Configure reset mode, initially stopped. Actual timeout depends on oscillator error.
    pub fn new(token: WatchdogToken, clocks: Clocks, config: Config) -> Result<Self, Error> {
        let source_hz = match config.clock {
            ClockSource::Rc32k => clocks.low_frequency_hz(),
            ClockSource::System => clocks.cpu_hz(),
        };
        let micros = u64::try_from(config.timeout.as_nanos().div_ceil(1000))
            .map_err(|_| Error::InvalidConfig)?;
        let ticks = crate::clock_logic::watchdog_match(source_hz, config.divider, micros)?;
        critical_section::with(|_| {
            pac::GLB.cgen_cfg1().modify(|r| r.set_timer(true));
            unlock();
            pac::TIMER.WMER().write(|r| r.0 = 2); // WE=0, WRIE=1 reset mode
            pac::TIMER.TCCR().modify(|r| {
                r.set_cs_wdt(match config.clock {
                    ClockSource::Rc32k => 1,
                    ClockSource::System => 0,
                })
            });
            pac::TIMER
                .TCDR()
                .modify(|r| r.set_wcdr((config.divider - 1) as u8));
            unlock();
            pac::TIMER.WCR().write(|r| r.0 = 1);
            unlock();
            pac::TIMER.WMR().write(|r| r.0 = u32::from(ticks));
            crate::io_fence();
        });
        Ok(Self {
            _token: token,
            match_ticks: ticks,
            source_hz,
            divider: config.divider,
        })
    }
    pub fn start(&mut self) {
        critical_section::with(|_| {
            unlock();
            pac::TIMER.WCR().write(|r| r.0 = 1);
            unlock();
            pac::TIMER.WMER().write(|r| r.0 = 3);
            crate::io_fence();
        });
    }
    pub fn feed(&mut self) {
        critical_section::with(|_| {
            unlock();
            pac::TIMER.WCR().write(|r| r.0 = 1);
            crate::io_fence();
        });
    }
    /// Explicitly stop; dropping the Rust object alone does not disable reset protection.
    pub fn stop(&mut self) {
        critical_section::with(|_| {
            unlock();
            pac::TIMER.WMER().write(|r| r.0 = 2);
            crate::io_fence();
        });
    }
    pub fn is_running(&self) -> bool {
        critical_section::with(|_| {
            unlock();
            pac::TIMER.WMER().read().0 & 1 != 0
        })
    }
    pub fn counter(&self) -> u16 {
        critical_section::with(|_| {
            unlock();
            pac::TIMER.WVR().read().0 as u16
        })
    }
    pub fn nominal_timeout(&self) -> Duration {
        Duration::from_micros(
            (u64::from(self.match_ticks) * u64::from(self.divider) * 1_000_000)
                .div_ceil(u64::from(self.source_hz)),
        )
    }
}
