//! Single-ended, finite ADC scans. No DMA or automatic heater control.
pub use crate::peripheral_logic::{Error, GainCalibration, Resolution};
use crate::{
    clock::Clocks,
    gpio, interrupt, pac,
    peripheral_logic::{AdcHardware, AdcTransfer, scan_words},
};
use core::{
    cell::{Cell, RefCell},
    future::poll_fn,
    task::{Poll, Waker},
};
use embassy_time::{Duration, Timer, with_timeout};

pub struct AdcToken {
    _private: (),
}
impl AdcToken {
    pub(crate) const fn new() -> Self {
        Self { _private: () }
    }
}
mod sealed {
    pub trait Sealed {}
}
pub struct AnalogPin<const N: u8> {
    _pin: gpio::Pin<N>,
}
pub trait Inputs: sealed::Sealed + Send {
    #[doc(hidden)]
    const MASK: u16;
}
macro_rules! analog {
    ($($pin:literal => $channel:literal),* $(,)?) => { $(
        impl gpio::Pin<$pin> {
            pub fn into_analog(self) -> AnalogPin<$pin> {
                self.configure(10,gpio::Pull::None,false); AnalogPin { _pin: self }
            }
        }
        impl sealed::Sealed for AnalogPin<$pin> {}
        impl Inputs for AnalogPin<$pin> { const MASK: u16 = 1 << $channel; }
    )* };
}
analog!(18=>8,19=>9,20=>10);
impl sealed::Sealed for () {}
impl Inputs for () {
    const MASK: u16 = 0;
}
impl<A: Inputs, B: Inputs> sealed::Sealed for (A, B) {}
impl<A: Inputs, B: Inputs> Inputs for (A, B) {
    const MASK: u16 = A::MASK | B::MASK;
}
impl<A: Inputs, B: Inputs, C: Inputs> sealed::Sealed for (A, B, C) {}
impl<A: Inputs, B: Inputs, C: Inputs> Inputs for (A, B, C) {
    const MASK: u16 = A::MASK | B::MASK | C::MASK;
}

#[derive(Clone, Copy, Debug)]
pub enum Channel {
    Input8 = 8,
    Input9 = 9,
    Input10 = 10,
    Ground = 23,
}
#[derive(Clone, Copy, Debug)]
pub enum Reference {
    V3_2,
    V2_0,
}
impl Reference {
    pub const fn microvolts(self) -> u32 {
        match self {
            Self::V3_2 => 3_200_000,
            Self::V2_0 => 2_000_000,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub resolution: Resolution,
    pub reference: Reference,
    pub calibration: Option<GainCalibration>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            resolution: Resolution::Bits14Average16,
            reference: Reference::V3_2,
            calibration: None,
        }
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Sample {
    pub channel: u8,
    /// FIFO's left-aligned sample field, without channel tags.
    pub raw: u16,
    /// Right-aligned counts, gain-corrected only when calibrated is true.
    pub counts: u16,
    pub calibrated: bool,
}
impl Sample {
    /// ADC pin voltage estimate; excludes board divider/thermocouple/NTC conversion.
    pub fn microvolts(self, resolution: Resolution, reference: Reference) -> u32 {
        (u64::from(self.counts) * u64::from(reference.microvolts())
            / u64::from(resolution.max_count())) as u32
    }
}
impl GainCalibration {
    /// Read only the boot-loaded shadow. No reload, fuse programming or ROM calls.
    pub fn from_efuse_shadow() -> Result<Self, Error> {
        // SDK GLB_AHB_CLOCK_IP_EF_CTRL gate; no EF_CTRL/EF_DATA writes.
        critical_section::with(|_| pac::GLB.cgen_cfg1().modify(|r| r.0 |= 1 << 7));
        crate::io_fence();
        let control = pac::EF_CTRL.ef_if_ctrl_0().read();
        if control.ef_if_0_busy()
            || !control.ef_if_0_autoload_done()
            || !control.ef_clk_sahb_data_sel()
        {
            return Err(Error::ShadowUnavailable);
        }
        Self::from_efuse_word(pac::EF_DATA_0.ef_key_slot_5_w3().read().0)
    }
}
struct State {
    transfer: AdcTransfer,
    waker: Option<Waker>,
}
static STATE: critical_section::Mutex<RefCell<State>> =
    critical_section::Mutex::new(RefCell::new(State {
        transfer: AdcTransfer::new(),
        waker: None,
    }));
// GPIP mixed status/control: masks 12..15, W1C 8..10, FIFO_CLR bit1.
const MASKS: u32 = 0xf000;
static RESTART: critical_section::Mutex<Cell<crate::peripheral_logic::RestartGate>> =
    critical_section::Mutex::new(Cell::new(crate::peripheral_logic::RestartGate::new()));
fn restart_gate() -> crate::peripheral_logic::RestartGate {
    critical_section::with(|cs| RESTART.borrow(cs).get())
}
fn fifo_control(masks: u32, clear: bool) {
    pac::GPIP
        .gpadc_config()
        .write(|r| r.0 = masks | if clear { (7 << 8) | 2 } else { 0 });
    crate::io_fence();
    if clear {
        pac::GPIP.gpadc_config().write(|r| r.0 = masks);
        crate::io_fence();
    }
}
fn stop_conversion() {
    pac::AON
        .gpadc_reg_cmd()
        .modify(|r| r.set_gpadc_conv_start(false));
    crate::io_fence();
    critical_section::with(|cs| {
        let mut gate = RESTART.borrow(cs).get();
        gate.stopped(crate::time::now_ticks());
        RESTART.borrow(cs).set(gate);
    });
    fifo_control(MASKS, false);
    interrupt::disable(interrupt::ADC);
    crate::io_fence();
}
struct Hardware;
impl AdcHardware for Hardware {
    fn error(&self) -> Option<Error> {
        let status = pac::GPIP.gpadc_config().read();
        let analog = pac::AON.gpadc_reg_isr().read();
        if status.gpadc_fifo_overrun() {
            Some(Error::FifoOverrun)
        } else if status.gpadc_fifo_underrun() {
            Some(Error::FifoUnderrun)
        } else if analog.gpadc_pos_satur() || analog.gpadc_neg_satur() {
            Some(Error::Saturated)
        } else {
            None
        }
    }
    fn count(&self) -> usize {
        usize::from(pac::GPIP.gpadc_config().read().gpadc_fifo_data_count())
    }
    fn pop(&mut self) -> u32 {
        pac::GPIP.gpadc_dma_rdata().read().0
    }
    fn stop(&mut self) {
        stop_conversion();
        fifo_control(MASKS, true);
    }
}
fn cancel() {
    let waker = critical_section::with(|cs| {
        let mut state = STATE.borrow(cs).borrow_mut();
        state.transfer.cancel(&mut Hardware);
        state.waker.take()
    });
    drop(waker);
}
fn dummy_wait() {
    for _ in 0..8 {
        unsafe {
            core::arch::asm!("nop");
        }
    }
}

pub struct Adc<P: Inputs> {
    _token: AdcToken,
    _pins: P,
    config: Config,
    channels: [u8; 12],
    len: usize,
    read: usize,
}
impl<P: Inputs> Adc<P> {
    /// Fixed nominal ADC clock: XCLK 32MHz / GLB 4 / ADC 4 = 2MHz maximum.
    /// Analog settings follow Pinecil's PGA x1/chop configuration, not a new precision claim.
    pub fn new(token: AdcToken, pins: P, clocks: Clocks, config: Config) -> Self {
        critical_section::with(|_| {
            // SDK GLB_AHB_CLOCK_IP_GPIP gate. Preserve all sibling gates.
            pac::GLB.cgen_cfg1().modify(|r| r.0 |= 1 << 2);
            pac::GLB
                .GPADC_32M_SRC_CTRL()
                .modify(|r| r.set_gpadc_32m_div_en(false));
            pac::GLB.GPADC_32M_SRC_CTRL().modify(|r| {
                r.set_gpadc_32m_clk_sel(true);
                r.set_gpadc_32m_clk_div((clocks.xclk_hz() / 8_000_000 - 1) as u8);
                r.set_gpadc_32m_div_en(true);
            });
            pac::HBN.HBN_PIR_CFG().modify(|r| r.set_gpadc_cgen(true));
            pac::AON
                .gpadc_reg_cmd()
                .write(|r| r.set_gpadc_global_en(true));
            pac::AON
                .gpadc_reg_cmd()
                .modify(|r| r.set_gpadc_soft_rst(true));
            dummy_wait();
            pac::AON
                .gpadc_reg_cmd()
                .modify(|r| r.set_gpadc_soft_rst(false));
            pac::AON.gpadc_reg_config1().write(|r| {
                r.set_gpadc_v18_sel(2);
                r.set_gpadc_v11_sel(1);
                r.set_gpadc_clk_div_ratio(1);
                r.set_gpadc_res_sel(config.resolution.encoding());
            });
            dummy_wait();
            pac::AON.gpadc_reg_config2().write(|r| {
                r.set_gpadc_dly_sel(2);
                r.set_gpadc_pga1_gain(1);
                r.set_gpadc_pga2_gain(1);
                r.set_gpadc_chop_mode(2);
                r.set_gpadc_pga_en(true);
                r.set_gpadc_pga_os_cal(8);
                r.set_gpadc_pga_vcm(1);
                r.set_gpadc_vref_sel(matches!(config.reference, Reference::V2_0));
            });
            pac::AON
                .gpadc_reg_define()
                .write(|r| r.set_gpadc_os_cal_data(0));
        });
        cancel();
        Self {
            _token: token,
            _pins: pins,
            config,
            channels: [0; 12],
            len: 0,
            read: 0,
        }
    }
    pub fn config(&self) -> Config {
        self.config
    }
    fn numbers(&self, channels: &[Channel]) -> Result<[u8; 12], Error> {
        if channels.is_empty() || channels.len() > 12 {
            return Err(Error::InvalidConfig);
        }
        let mut numbers = [0; 12];
        for (i, &channel) in channels.iter().enumerate() {
            let n = channel as u8;
            if n < 12 && P::MASK & (1 << n) == 0 {
                return Err(Error::InvalidConfig);
            }
            numbers[i] = n;
        }
        Ok(numbers)
    }
    /// Stop, clear and configure a finite scan. Wait >=100us before start_prepared.
    pub fn prepare_scan(&mut self, channels: &[Channel]) -> Result<(), Error> {
        let numbers = self.numbers(channels)?;
        if critical_section::with(|cs| STATE.borrow(cs).borrow().transfer.active) {
            return Err(Error::Busy);
        }
        self.prepare(&numbers[..channels.len()]);
        self.channels = numbers;
        self.len = channels.len();
        self.read = 0;
        Ok(())
    }
    fn prepare(&mut self, numbers: &[u8]) {
        stop_conversion();
        fifo_control(MASKS, true);
        // Mask saturation IRQs and explicitly clear both stale W1C flags.
        pac::AON.gpadc_reg_isr().write(|r| {
            r.set_gpadc_pos_satur_mask(true);
            r.set_gpadc_neg_satur_mask(true);
            r.set_gpadc_pos_satur_clr(true);
            r.set_gpadc_neg_satur_clr(true);
        });
        let positive = scan_words(numbers).unwrap();
        let ground = scan_words(&[23; 12]).unwrap();
        pac::AON.gpadc_reg_scn_pos1().write(|r| r.0 = positive[0]);
        pac::AON.gpadc_reg_scn_pos2().write(|r| r.0 = positive[1]);
        pac::AON.gpadc_reg_scn_neg1().write(|r| r.0 = ground[0]);
        pac::AON.gpadc_reg_scn_neg2().write(|r| r.0 = ground[1]);
        pac::AON.gpadc_reg_config1().modify(|r| {
            r.set_gpadc_scan_en(true);
            r.set_gpadc_scan_length((numbers.len() - 1) as u8);
            r.set_gpadc_clk_ana_inv(true);
            r.set_gpadc_cont_conv_en(false);
        });
    }
    /// Nonblocking trigger for timer ISR use. Busy means the SDK restart pause has not elapsed.
    pub fn start_prepared(&mut self) -> Result<(), Error> {
        if self.len == 0 {
            return Err(Error::InvalidConfig);
        }
        if self.is_running()
            || critical_section::with(|cs| STATE.borrow(cs).borrow().transfer.active)
        {
            return Err(Error::Busy);
        }
        if !restart_gate().can_start(crate::time::now_ticks()) {
            return Err(Error::Busy);
        }
        self.read = 0;
        pac::AON
            .gpadc_reg_cmd()
            .modify(|r| r.set_gpadc_conv_start(true));
        crate::io_fence();
        Ok(())
    }
    pub fn is_running(&self) -> bool {
        pac::AON.gpadc_reg_cmd().read().gpadc_conv_start()
    }
    /// Stop and mask IRQs, preserving FIFO for synchronous drain by the owner.
    pub fn stop(&mut self) {
        stop_conversion();
    }
    /// Abort even a forgotten async scan; clears FIFO and completion state.
    pub fn abort(&mut self) {
        cancel();
    }
    pub fn try_read(&mut self) -> Result<Option<Sample>, Error> {
        if critical_section::with(|cs| STATE.borrow(cs).borrow().transfer.active) {
            return Err(Error::Busy);
        }
        if let Some(error) = Hardware.error() {
            return Err(error);
        }
        if Hardware.count() == 0 {
            return Ok(None);
        }
        if self.read >= self.len {
            return Err(Error::ExtraData);
        }
        let word = Hardware.pop();
        if ((word >> 21) & 31) as u8 != self.channels[self.read] || ((word >> 16) & 31) != 23 {
            return Err(Error::UnexpectedChannel);
        }
        self.read += 1;
        Ok(Some(self.sample(word)))
    }
    fn sample(&self, word: u32) -> Sample {
        Sample {
            channel: ((word >> 21) & 31) as u8,
            raw: word as u16,
            counts: self
                .config
                .calibration
                .unwrap_or(GainCalibration::UNITY)
                .apply(self.config.resolution.decode(word), self.config.resolution),
            calibrated: self.config.calibration.is_some(),
        }
    }
    /// Complete finite scan through IRQ 41. Caller output is unchanged on error/cancel.
    pub async fn read_scan(
        &mut self,
        channels: &[Channel],
        output: &mut [Sample],
        timeout: Duration,
    ) -> Result<(), Error> {
        match with_timeout(timeout, self.scan_inner(channels, output)).await {
            Ok(result) => result,
            Err(_) => Err(Error::Timeout),
        }
    }
    async fn scan_inner(
        &mut self,
        channels: &[Channel],
        output: &mut [Sample],
    ) -> Result<(), Error> {
        if output.len() != channels.len() {
            return Err(Error::InvalidConfig);
        }
        self.prepare_scan(channels)?;
        critical_section::with(|cs| {
            STATE
                .borrow(cs)
                .borrow_mut()
                .transfer
                .begin(&self.channels[..self.len])
        })?;
        struct Guard;
        impl Drop for Guard {
            fn drop(&mut self) {
                cancel();
            }
        }
        let _guard = Guard;
        Timer::at(embassy_time::Instant::from_ticks(restart_gate().ready_at())).await;
        critical_section::with(|_| {
            fifo_control(1 << 12, false); // FIFO threshold=1 word; error/FIFO-ready IRQs unmasked.
            unsafe {
                interrupt::enable(interrupt::ADC);
            }
            pac::AON
                .gpadc_reg_cmd()
                .modify(|r| r.set_gpadc_conv_start(true));
            crate::io_fence();
        });
        poll_fn(|cx| {
            let mut waker = Some(cx.waker().clone());
            let result = critical_section::with(|cs| {
                let mut state = STATE.borrow(cs).borrow_mut();
                if let Some(result) = state.transfer.result() {
                    return Some(result);
                }
                core::mem::swap(&mut state.waker, &mut waker);
                None
            });
            drop(waker);
            match result {
                Some(result) => Poll::Ready(result),
                None => Poll::Pending,
            }
        })
        .await?;
        let words = critical_section::with(|cs| {
            let state = STATE.borrow(cs).borrow();
            let mut words = [0; 12];
            words[..self.len].copy_from_slice(state.transfer.words());
            words
        });
        for (sample, word) in output.iter_mut().zip(words) {
            *sample = self.sample(word);
        }
        Ok(())
    }
}
impl<P: Inputs> Drop for Adc<P> {
    fn drop(&mut self) {
        cancel();
        pac::AON
            .gpadc_reg_cmd()
            .modify(|r| r.set_gpadc_global_en(false));
    }
}
pub(crate) fn on_interrupt() {
    let waker = critical_section::with(|cs| {
        let mut state = STATE.borrow(cs).borrow_mut();
        if state.transfer.on_interrupt(&mut Hardware) {
            state.waker.take()
        } else {
            None
        }
    });
    if let Some(waker) = waker {
        waker.wake();
    }
}
