//! BL70x v1 packed pin configuration, polled inputs and outputs.
use crate::pac;
pub struct Pin<const N: u8> {
    _private: (),
}
pub struct Input<const N: u8> {
    _pin: Pin<N>,
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
            | (1 << 1)
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
    /// Level wait sampled every millisecond; no GPIO interrupt is claimed.
    pub async fn wait_for_low(&mut self) {
        while self.is_high() {
            embassy_time::Timer::after_millis(1).await;
        }
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
