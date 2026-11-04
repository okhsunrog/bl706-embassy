//! Frozen clock planning, independent of MMIO.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reference {
    Rc32m,
    Xtal32m,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Root {
    Rc32m,
    Xtal32m,
    Dll96m(Reference),
    Dll144m(Reference),
}
#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub root: Root,
    /// Actual divisors, 1..=256. BCLK is divided from FCLK/CPU.
    pub cpu_divider: u16,
    pub bus_divider: u16,
    pub xtal_capcode_in: u8,
    pub xtal_capcode_out: u8,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            root: Root::Rc32m,
            cpu_divider: 1,
            bus_divider: 1,
            xtal_capcode_in: 33,
            xtal_capcode_out: 33,
        }
    }
}
impl Config {
    pub fn pinecil_v2_144mhz() -> Self {
        Self {
            root: Root::Dll144m(Reference::Xtal32m),
            bus_divider: 2,
            ..Self::default()
        }
    }
    pub(crate) fn plan(self) -> Result<Clocks, Error> {
        if !(1..=256).contains(&self.cpu_divider)
            || !(1..=256).contains(&self.bus_divider)
            || self.xtal_capcode_in > 63
            || self.xtal_capcode_out > 63
        {
            return Err(Error::InvalidConfig);
        }
        let root = match self.root {
            Root::Rc32m | Root::Xtal32m => 32_000_000,
            Root::Dll96m(_) => 96_000_000,
            Root::Dll144m(_) => 144_000_000,
        };
        let cpu = root / u32::from(self.cpu_divider);
        let bus = cpu / u32::from(self.bus_divider);
        if root % u32::from(self.cpu_divider) != 0
            || cpu % u32::from(self.bus_divider) != 0
            || cpu > 160_000_000
            || !(1_000_000..=80_000_000).contains(&bus)
            || bus % 1_000_000 != 0
        {
            return Err(Error::InvalidConfig);
        }
        Ok(Clocks {
            cpu,
            bus,
            root: self.root,
        })
    }
    pub(crate) fn uses_xtal(self) -> bool {
        matches!(
            self.root,
            Root::Xtal32m | Root::Dll96m(Reference::Xtal32m) | Root::Dll144m(Reference::Xtal32m)
        )
    }
    pub(crate) fn root_encoding(self) -> u32 {
        match self.root {
            Root::Rc32m => 0,
            Root::Xtal32m => 1,
            Root::Dll96m(Reference::Rc32m) | Root::Dll144m(Reference::Rc32m) => 2,
            _ => 3,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidConfig,
    XtalTimeout,
    AlreadyInitialized,
}
#[derive(Clone, Copy, Debug)]
pub struct Clocks {
    cpu: u32,
    bus: u32,
    root: Root,
}
impl Clocks {
    pub const fn cpu_hz(self) -> u32 {
        self.cpu
    }
    pub const fn bus_hz(self) -> u32 {
        self.bus
    }
    pub const fn uart_hz(self) -> u32 {
        self.cpu
    }
    pub const fn xclk_hz(self) -> u32 {
        32_000_000
    }
    pub const fn low_frequency_hz(self) -> u32 {
        32_000
    }
    pub const fn root(self) -> Root {
        self.root
    }
    pub(crate) const fn mtime_divider(self) -> u16 {
        (self.bus / 1_000_000) as u16
    }
    pub(crate) fn i2c_phase(self) -> u8 {
        (self.bus.div_ceil(400_000) - 1) as u8
    }
}
pub(crate) fn watchdog_match(source_hz: u32, divider: u16, timeout_us: u64) -> Result<u16, Error> {
    if !(1..=256).contains(&divider) || timeout_us == 0 {
        return Err(Error::InvalidConfig);
    }
    let ticks =
        (u128::from(timeout_us) * u128::from(source_hz)).div_ceil(u128::from(divider) * 1_000_000);
    if ticks == 0 || ticks > 65535 {
        return Err(Error::InvalidConfig);
    }
    Ok(ticks as u16)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn supported_profiles_and_peripheral_rates() {
        for (config, cpu, bus) in [
            (Config::default(), 32_000_000, 32_000_000),
            (
                Config {
                    root: Root::Xtal32m,
                    ..Config::default()
                },
                32_000_000,
                32_000_000,
            ),
            (
                Config {
                    root: Root::Dll96m(Reference::Rc32m),
                    bus_divider: 2,
                    ..Config::default()
                },
                96_000_000,
                48_000_000,
            ),
            (Config::pinecil_v2_144mhz(), 144_000_000, 72_000_000),
        ] {
            let clocks = config.plan().unwrap();
            assert_eq!(clocks.cpu_hz(), cpu);
            assert_eq!(clocks.bus_hz(), bus);
            assert_eq!(clocks.uart_hz(), cpu);
            assert_eq!(clocks.xclk_hz(), 32_000_000);
            assert_eq!(u32::from(clocks.mtime_divider()) * 1_000_000, bus);
            assert!(bus / (4 * (u32::from(clocks.i2c_phase()) + 1)) <= 100_000);
        }
    }
    #[test]
    fn divider_limits_and_no_bus_overclock() {
        assert!(
            Config {
                root: Root::Dll144m(Reference::Rc32m),
                ..Config::default()
            }
            .plan()
            .is_err()
        );
        for div in [0, 257, u16::MAX] {
            assert!(
                Config {
                    cpu_divider: div,
                    ..Config::default()
                }
                .plan()
                .is_err()
            );
        }
        assert!(
            Config {
                cpu_divider: 3,
                ..Config::default()
            }
            .plan()
            .is_err()
        );
        assert!(
            Config {
                bus_divider: 64,
                ..Config::default()
            }
            .plan()
            .is_err()
        );
        assert!(
            Config {
                xtal_capcode_in: 64,
                ..Config::default()
            }
            .plan()
            .is_err()
        );
        assert_eq!(
            Config {
                bus_divider: 32,
                ..Config::default()
            }
            .plan()
            .unwrap()
            .mtime_divider(),
            1
        );
    }
    #[test]
    fn mux_reference_is_preserved_for_dll() {
        assert_eq!(Config::default().root_encoding(), 0);
        assert_eq!(Config::pinecil_v2_144mhz().root_encoding(), 3);
        assert!(Config::pinecil_v2_144mhz().uses_xtal());
        assert_eq!(
            Config {
                root: Root::Dll96m(Reference::Rc32m),
                ..Config::default()
            }
            .root_encoding(),
            2
        );
    }
    #[test]
    fn watchdog_rounds_up_and_rejects_unrepresentable_timeouts() {
        assert_eq!(watchdog_match(32_000, 32, 2_000_000), Ok(2000));
        assert_eq!(watchdog_match(32_000, 1, 1), Ok(1));
        assert_eq!(watchdog_match(32_000_000, 256, 524_280), Ok(65535));
        assert!(watchdog_match(32_000_000, 256, 524_281).is_err());
        assert!(watchdog_match(32_000, 0, 1).is_err());
        assert!(watchdog_match(32_000, 1, 0).is_err());
    }
}
