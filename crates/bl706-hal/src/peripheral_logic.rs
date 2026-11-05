//! Register-independent validation and bounded ADC IRQ engine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidConfig,
    Busy,
    Timeout,
    Disabled,
    FifoOverrun,
    FifoUnderrun,
    UnexpectedChannel,
    ExtraData,
    ShortScan,
    InvalidCalibration,
    ShadowUnavailable,
    Saturated,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Resolution {
    Bits12,
    Bits14Average16,
    Bits14Average64,
    Bits16Average128,
    Bits16Average256,
}
impl Resolution {
    pub(crate) const fn encoding(self) -> u8 {
        self as u8
    }
    pub const fn bits(self) -> u8 {
        match self {
            Self::Bits12 => 12,
            Self::Bits14Average16 | Self::Bits14Average64 => 14,
            _ => 16,
        }
    }
    pub const fn max_count(self) -> u16 {
        u16::MAX >> (16 - self.bits())
    }
    pub fn decode(self, word: u32) -> u16 {
        (word as u16) >> (16 - self.bits())
    }
}

/// Integer representation of SDK coe = 1 - signed_trim / 2048.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GainCalibration {
    denominator: u16,
}
impl GainCalibration {
    pub const UNITY: Self = Self { denominator: 2048 };
    pub fn from_efuse_word(word: u32) -> Result<Self, Error> {
        let trim = (word >> 1) & 0xfff;
        if word & (1 << 14) == 0 || (trim.count_ones() & 1) != ((word >> 13) & 1) {
            return Err(Error::InvalidCalibration);
        }
        let signed = ((trim << 20) as i32) >> 20;
        Ok(Self {
            denominator: (2048 - signed) as u16,
        })
    }
    pub fn apply(self, counts: u16, resolution: Resolution) -> u16 {
        ((u32::from(counts) * 2048 / u32::from(self.denominator))
            .min(u32::from(resolution.max_count()))) as u16
    }
}

pub(crate) fn pwm_valid(divider: u16, period: u16, duty: u16) -> Result<(), Error> {
    if divider == 0 || period == 0 || duty > period {
        Err(Error::InvalidConfig)
    } else {
        Ok(())
    }
}
pub(crate) fn replace_field(word: u32, mask: u32, shift: u8, value: u32) -> u32 {
    (word & !(mask << shift)) | ((value & mask) << shift)
}
pub(crate) fn timer_valid(divider: u16, preload_match: Option<(u32, u32)>) -> Result<(), Error> {
    if divider == 0
        || divider > 256
        || preload_match.is_some_and(|(preload, compare)| compare <= preload)
    {
        Err(Error::InvalidConfig)
    } else {
        Ok(())
    }
}
pub(crate) fn scan_words(channels: &[u8]) -> Result<[u32; 2], Error> {
    if channels.is_empty() || channels.len() > 12 || channels.iter().any(|&n| n > 11 && n != 23) {
        return Err(Error::InvalidConfig);
    }
    let mut words = [0; 2];
    for (index, &channel) in channels.iter().enumerate() {
        words[index / 6] |= u32::from(channel) << ((index % 6) * 5);
    }
    Ok(words)
}

#[derive(Clone, Copy)]
pub(crate) struct RestartGate {
    ready_at: u64,
}
impl RestartGate {
    pub(crate) const fn new() -> Self {
        Self { ready_at: 0 }
    }
    pub(crate) fn stopped(&mut self, now: u64) {
        self.ready_at = now.saturating_add(100);
    }
    pub(crate) fn ready_at(self) -> u64 {
        self.ready_at
    }
    pub(crate) fn can_start(self, now: u64) -> bool {
        now >= self.ready_at
    }
}

pub(crate) trait AdcHardware {
    fn error(&self) -> Option<Error>;
    fn count(&self) -> usize;
    fn pop(&mut self) -> u32;
    /// Stop conversions, mask IRQs, clear status and FIFO before publishing result.
    fn stop(&mut self);
}
pub(crate) struct AdcTransfer {
    channels: [u8; 12],
    words: [u32; 12],
    len: usize,
    done: usize,
    pub(crate) active: bool,
    result: Option<Result<(), Error>>,
}
impl AdcTransfer {
    pub(crate) const fn new() -> Self {
        Self {
            channels: [0; 12],
            words: [0; 12],
            len: 0,
            done: 0,
            active: false,
            result: None,
        }
    }
    pub(crate) fn begin(&mut self, channels: &[u8]) -> Result<(), Error> {
        scan_words(channels)?;
        if self.active {
            return Err(Error::Busy);
        }
        self.channels[..channels.len()].copy_from_slice(channels);
        self.len = channels.len();
        self.done = 0;
        self.result = None;
        self.active = true;
        Ok(())
    }
    fn finish(&mut self, hw: &mut impl AdcHardware, result: Result<(), Error>) -> bool {
        hw.stop();
        self.active = false;
        self.result = Some(result);
        true
    }
    pub(crate) fn on_interrupt(&mut self, hw: &mut impl AdcHardware) -> bool {
        if !self.active {
            hw.stop();
            return false;
        }
        if let Some(error) = hw.error() {
            return self.finish(hw, Err(error));
        }
        let count = hw.count();
        if count > self.len - self.done {
            return self.finish(hw, Err(Error::ExtraData));
        }
        // At most twelve FIFO reads per IRQ; no spin until a conversion finishes.
        for _ in 0..count {
            let word = hw.pop();
            if ((word >> 21) & 31) as u8 != self.channels[self.done] || ((word >> 16) & 31) != 23 {
                return self.finish(hw, Err(Error::UnexpectedChannel));
            }
            self.words[self.done] = word;
            self.done += 1;
        }
        if let Some(error) = hw.error() {
            return self.finish(hw, Err(error));
        }
        if self.done == self.len {
            return self.finish(hw, Ok(()));
        }
        false
    }
    pub(crate) fn result(&self) -> Option<Result<(), Error>> {
        self.result
    }
    pub(crate) fn words(&self) -> &[u32] {
        &self.words[..self.len]
    }
    pub(crate) fn cancel(&mut self, hw: &mut impl AdcHardware) {
        hw.stop();
        self.active = false;
        self.result = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pwm_bounds_and_full_duty() {
        for period in [1, 100, 65535] {
            assert!(pwm_valid(1, period, 0).is_ok());
            assert!(pwm_valid(65535, period, period).is_ok());
        }
        assert!(pwm_valid(0, 100, 1).is_err());
        assert!(pwm_valid(1, 0, 0).is_err());
        assert!(pwm_valid(1, 100, 101).is_err());
    }
    #[test]
    fn shared_timer_fields_preserve_sibling_and_watchdog() {
        for (shift, mask) in [(2, 3), (5, 3), (8, 255), (16, 255), (1, 1), (2, 1)] {
            let old = 0xa5a5_a5a5;
            let new = replace_field(old, mask, shift, 1);
            assert_eq!(new & !(mask << shift), old & !(mask << shift));
            assert_eq!((new >> shift) & mask, 1);
        }
    }
    #[test]
    fn timer_divisor_and_preload_validation() {
        for divisor in [1, 2, 32, 256] {
            assert!(timer_valid(divisor, None).is_ok());
        }
        for divisor in [0, 257, u16::MAX] {
            assert!(timer_valid(divisor, None).is_err());
        }
        assert!(timer_valid(1, Some((0, 1))).is_ok());
        assert!(timer_valid(256, Some((u32::MAX - 1, u32::MAX))).is_ok());
        assert!(timer_valid(1, Some((1, 1))).is_err());
        assert!(timer_valid(1, Some((10, 9))).is_err());
    }
    #[test]
    fn adc_restart_gate_rearms_after_every_stop_not_only_scan_preparation() {
        let mut gate = RestartGate::new();
        for stopped_at in [10, 120, 10_000, 1_000_000] {
            gate.stopped(stopped_at);
            assert_eq!(gate.ready_at(), stopped_at + 100);
            assert!(!gate.can_start(stopped_at));
            assert!(!gate.can_start(stopped_at + 99));
            assert!(gate.can_start(stopped_at + 100));
        }
    }
    #[test]
    fn scan_packing_all_lengths_and_repeated_channels() {
        for len in 1..=12 {
            let channels: std::vec::Vec<_> = (0..len).map(|n| [8, 9, 10, 23][n % 4]).collect();
            let words = scan_words(&channels).unwrap();
            for (i, &c) in channels.iter().enumerate() {
                assert_eq!((words[i / 6] >> (5 * (i % 6))) & 31, u32::from(c));
            }
        }
        assert!(scan_words(&[]).is_err());
        assert!(scan_words(&[0; 13]).is_err());
        assert!(scan_words(&[12]).is_err());
    }
    #[test]
    fn calibration_all_signed_values_parity_and_clamping() {
        for trim in 0u32..4096 {
            let word = (1 << 14) | ((trim.count_ones() & 1) << 13) | (trim << 1);
            let cal = GainCalibration::from_efuse_word(word).unwrap();
            let signed = ((trim << 20) as i32) >> 20;
            assert_eq!(u32::from(cal.denominator), (2048 - signed) as u32);
            assert!(GainCalibration::from_efuse_word(word ^ (1 << 13)).is_err());
            assert!(GainCalibration::from_efuse_word(word & !(1 << 14)).is_err());
            assert!(cal.apply(65535, Resolution::Bits14Average16) <= 16383);
        }
        assert_eq!(GainCalibration::UNITY.apply(1234, Resolution::Bits12), 1234);
    }
    #[test]
    fn adc_resolution_is_left_aligned_and_channel_bits_are_not_samples() {
        for res in [
            Resolution::Bits12,
            Resolution::Bits14Average16,
            Resolution::Bits14Average64,
            Resolution::Bits16Average128,
            Resolution::Bits16Average256,
        ] {
            assert_eq!(
                res.decode((10 << 21) | (23 << 16) | 0xffff),
                res.max_count()
            );
            assert_eq!(res.decode((10 << 21) | (23 << 16)), 0);
            assert_eq!(res.encoding(), res as u8);
        }
    }
    struct Hw {
        fifo: std::collections::VecDeque<u32>,
        error: Option<Error>,
        late: Option<Error>,
        reads: usize,
        stopped: bool,
    }
    impl Hw {
        fn new() -> Self {
            Self {
                fifo: std::collections::VecDeque::new(),
                error: None,
                late: None,
                reads: 0,
                stopped: false,
            }
        }
        fn feed(&mut self, channel: u8) {
            self.fifo
                .push_back((u32::from(channel) << 21) | (23 << 16) | 123);
        }
    }
    impl AdcHardware for Hw {
        fn error(&self) -> Option<Error> {
            self.error.or(if self.reads > 0 { self.late } else { None })
        }
        fn count(&self) -> usize {
            self.fifo.len()
        }
        fn pop(&mut self) -> u32 {
            self.reads += 1;
            self.fifo.pop_front().unwrap()
        }
        fn stop(&mut self) {
            self.stopped = true;
            self.fifo.clear();
        }
    }
    #[test]
    fn adc_all_scan_lengths_progress_without_waiter_polls() {
        for len in 1..=12 {
            let mut transfer = AdcTransfer::new();
            let mut hw = Hw::new();
            let channels = [9; 12];
            transfer.begin(&channels[..len]).unwrap();
            for i in 0..len {
                hw.feed(9);
                assert_eq!(transfer.on_interrupt(&mut hw), i + 1 == len);
                assert_eq!(transfer.result().is_some(), i + 1 == len);
            }
            assert_eq!(transfer.result(), Some(Ok(())));
            assert!(hw.stopped);
            assert_eq!(transfer.words().len(), len);
            assert!(!transfer.on_interrupt(&mut hw));
            assert_eq!(transfer.result(), Some(Ok(())));
        }
    }
    #[test]
    fn adc_errors_win_over_completion_and_quiesce() {
        for error in [Error::FifoOverrun, Error::FifoUnderrun, Error::Saturated] {
            for late in [false, true] {
                let mut transfer = AdcTransfer::new();
                let mut hw = Hw::new();
                transfer.begin(&[9]).unwrap();
                hw.feed(9);
                if late {
                    hw.late = Some(error);
                } else {
                    hw.error = Some(error);
                }
                assert!(transfer.on_interrupt(&mut hw));
                assert_eq!(transfer.result(), Some(Err(error)));
                assert!(hw.stopped);
            }
        }
    }
    #[test]
    fn adc_mismatched_extra_data_cancel_and_reuse() {
        let mut transfer = AdcTransfer::new();
        let mut hw = Hw::new();
        transfer.begin(&[9]).unwrap();
        assert_eq!(transfer.begin(&[9]), Err(Error::Busy));
        hw.feed(8);
        transfer.on_interrupt(&mut hw);
        assert_eq!(transfer.result(), Some(Err(Error::UnexpectedChannel)));
        transfer.begin(&[9]).unwrap();
        hw.feed(9);
        hw.feed(9);
        transfer.on_interrupt(&mut hw);
        assert_eq!(transfer.result(), Some(Err(Error::ExtraData)));
        transfer.begin(&[9, 9]).unwrap();
        hw.feed(9);
        transfer.on_interrupt(&mut hw);
        transfer.cancel(&mut hw);
        assert!(!transfer.active);
        assert_eq!(transfer.result(), None);
        assert!(!transfer.on_interrupt(&mut hw));
        transfer.begin(&[8]).unwrap();
        hw.feed(8);
        transfer.on_interrupt(&mut hw);
        assert_eq!(transfer.result(), Some(Ok(())));
    }
}
