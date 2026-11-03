//! One-shot GPIO IRQ state, also exercised by the host register model.
#[derive(Clone, Copy)]
pub(crate) enum Trigger {
    Low,
    High,
    Rising,
    Falling,
    AnyEdge,
}

impl Trigger {
    fn modes(self) -> (u32, Option<u32>) {
        // SDK GLB_GPIO_INT_CONTROL_ASYNC | GLB_GPIO_INT_TRIG_*.
        match self {
            Self::Low => (6, None),
            Self::High => (7, None),
            Self::Rising => (5, None),
            Self::Falling => (4, None),
            Self::AnyEdge => (5, Some(4)),
        }
    }
    fn matches(self, high: bool) -> bool {
        matches!((self, high), (Self::Low, false) | (Self::High, true))
    }
}

pub(crate) fn mode_word(old: u32, pin: u8, mode: u32) -> u32 {
    let shift = (pin % 10) * 3;
    (old & !(7 << shift)) | (mode << shift)
}

pub(crate) trait Hardware {
    fn mask(&mut self, pins: u32);
    fn configure(&mut self, pin: u8, primary: u32, secondary: Option<u32>);
    fn clear(&mut self, pins: u32);
    fn unmask(&mut self, pin: u8, secondary: bool);
    fn high(&self, pin: u8) -> bool;
    /// OR of both banks' unmasked status.
    fn pending(&self) -> u32;
}

pub(crate) struct State {
    armed: u32,
    complete: u32,
}

impl State {
    pub(crate) const fn new() -> Self {
        Self {
            armed: 0,
            complete: 0,
        }
    }
    pub(crate) fn arm(&mut self, pin: u8, trigger: Trigger, hw: &mut impl Hardware) {
        let bit = 1 << pin;
        self.cancel(pin, hw);
        let (primary, secondary) = trigger.modes();
        hw.configure(pin, primary, secondary);
        // Discard stale events, including those caused by changing the mode.
        hw.clear(bit);
        if trigger.matches(hw.high(pin)) {
            self.complete |= bit;
            return;
        }
        self.armed |= bit;
        hw.unmask(pin, secondary.is_some());
        // Close the level-sample/unmask window. Edge events remain hardware-latched.
        if trigger.matches(hw.high(pin)) {
            self.finish(bit, hw);
        }
    }
    pub(crate) fn ready(&self, pin: u8) -> bool {
        self.complete & (1 << pin) != 0
    }
    fn finish(&mut self, bits: u32, hw: &mut impl Hardware) {
        // Mask before clear: an asserted level must not cause an interrupt storm.
        hw.mask(bits);
        hw.clear(bits);
        self.armed &= !bits;
        self.complete |= bits;
    }
    pub(crate) fn on_interrupt(&mut self, hw: &mut impl Hardware) -> u32 {
        let bits = hw.pending() & self.armed;
        if bits != 0 {
            self.finish(bits, hw);
        }
        bits
    }
    pub(crate) fn cancel(&mut self, pin: u8, hw: &mut impl Hardware) {
        let bit = 1 << pin;
        hw.mask(bit);
        hw.clear(bit);
        self.armed &= !bit;
        self.complete &= !bit;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Registers {
        mask: [u32; 2],
        status: [u32; 2],
        mode: [[u32; 4]; 2],
        inputs: u32,
        on_unmask: Option<(u8, bool)>,
    }
    impl Registers {
        fn new() -> Self {
            Self {
                mask: [u32::MAX; 2],
                status: [0; 2],
                mode: [[0; 4]; 2],
                inputs: 0,
                on_unmask: None,
            }
        }
        fn change(&mut self, pin: u8, high: bool) {
            let bit = 1 << pin;
            let old = self.high(pin);
            self.inputs = (self.inputs & !bit) | (u32::from(high) << pin);
            for bank in 0..2 {
                let mode = (self.mode[bank][usize::from(pin / 10)] >> ((pin % 10) * 3)) & 7;
                if match mode & 3 {
                    0 => old && !high,
                    1 => !old && high,
                    2 => !high,
                    _ => high,
                } {
                    self.status[bank] |= bit;
                }
            }
        }
    }
    impl Hardware for Registers {
        fn mask(&mut self, pins: u32) {
            for m in &mut self.mask {
                *m |= pins;
            }
        }
        fn configure(&mut self, pin: u8, primary: u32, secondary: Option<u32>) {
            let word = usize::from(pin / 10);
            self.mode[0][word] = mode_word(self.mode[0][word], pin, primary);
            self.mode[1][word] = mode_word(self.mode[1][word], pin, secondary.unwrap_or(4));
        }
        fn clear(&mut self, pins: u32) {
            for s in &mut self.status {
                *s &= !pins;
            }
        }
        fn unmask(&mut self, pin: u8, secondary: bool) {
            self.mask[0] &= !(1 << pin);
            if secondary {
                self.mask[1] &= !(1 << pin);
            }
            if let Some((p, high)) = self.on_unmask.take() {
                self.change(p, high);
            }
        }
        fn high(&self, pin: u8) -> bool {
            self.inputs & (1 << pin) != 0
        }
        fn pending(&self) -> u32 {
            (self.status[0] & !self.mask[0]) | (self.status[1] & !self.mask[1])
        }
    }

    #[test]
    fn mode_fields_preserve_neighbours_for_every_pin() {
        for pin in 0..32 {
            for mode in 4..8 {
                let old = 0xa5a5_a5a5;
                let shift = (pin % 10) * 3;
                let new = mode_word(old, pin, mode);
                assert_eq!((new >> shift) & 7, mode);
                assert_eq!(new & !(7 << shift), old & !(7 << shift));
            }
        }
    }
    #[test]
    fn level_waits_complete_immediately_only_at_requested_level() {
        for high in [false, true] {
            for trigger in [Trigger::Low, Trigger::High] {
                let mut hw = Registers::new();
                hw.change(16, high);
                let mut state = State::new();
                state.arm(16, trigger, &mut hw);
                assert_eq!(state.ready(16), trigger.matches(high));
                if !state.ready(16) {
                    hw.change(16, !high);
                    state.on_interrupt(&mut hw);
                }
                assert!(state.ready(16));
                assert_eq!(hw.mask, [u32::MAX; 2]);
                assert_eq!(state.on_interrupt(&mut hw), 0);
            }
        }
    }
    #[test]
    fn edge_is_latched_until_poll_and_old_status_is_discarded() {
        for pin in 0..32 {
            for (trigger, initial) in [(Trigger::Rising, false), (Trigger::Falling, true)] {
                let mut hw = Registers::new();
                hw.change(pin, initial);
                hw.status = [1 << pin; 2];
                let mut state = State::new();
                state.arm(pin, trigger, &mut hw);
                assert!(!state.ready(pin));
                assert_eq!(state.on_interrupt(&mut hw), 0);
                hw.change(pin, !initial);
                hw.change(pin, initial);
                assert_eq!(state.on_interrupt(&mut hw), 1 << pin);
                assert!(state.ready(pin));
                assert!(state.ready(pin)); // Repeated/spurious poll cannot lose completion.
                assert_eq!(state.on_interrupt(&mut hw), 0);
            }
        }
    }
    #[test]
    fn any_edge_uses_both_banks_and_completes_once() {
        for initial in [false, true] {
            let mut hw = Registers::new();
            hw.change(28, initial);
            let mut state = State::new();
            state.arm(28, Trigger::AnyEdge, &mut hw);
            assert!(!state.ready(28));
            hw.change(28, !initial);
            hw.change(28, initial);
            assert_eq!(hw.status, [1 << 28; 2]);
            assert_eq!(state.on_interrupt(&mut hw), 1 << 28);
            assert_eq!(hw.mask, [u32::MAX; 2]);
            assert_eq!(state.on_interrupt(&mut hw), 0);
        }
    }
    #[test]
    fn level_change_during_unmask_and_edge_before_first_poll_are_not_lost() {
        for trigger in [Trigger::High, Trigger::Rising] {
            let mut hw = Registers::new();
            hw.on_unmask = Some((16, true));
            let mut state = State::new();
            state.arm(16, trigger, &mut hw);
            state.on_interrupt(&mut hw);
            assert!(state.ready(16));
        }
    }
    #[test]
    fn simultaneous_pins_and_cancel_are_independent() {
        let mut hw = Registers::new();
        let mut state = State::new();
        for pin in [16, 25, 28] {
            state.arm(pin, Trigger::AnyEdge, &mut hw);
        }
        hw.change(16, true);
        state.cancel(16, &mut hw);
        hw.change(25, true);
        hw.change(28, true);
        assert_eq!(state.on_interrupt(&mut hw), (1 << 25) | (1 << 28));
        assert!(!state.ready(16));
        assert!(state.ready(25) && state.ready(28));
        state.arm(16, Trigger::Falling, &mut hw);
        assert!(!state.ready(16));
        hw.change(16, false);
        assert_eq!(state.on_interrupt(&mut hw), 1 << 16);
    }
    #[test]
    fn cancelled_or_forgotten_wait_can_be_rearmed_without_stale_completion() {
        let mut hw = Registers::new();
        let mut state = State::new();
        for complete_first in [false, true] {
            state.arm(10, Trigger::AnyEdge, &mut hw);
            hw.change(10, !hw.high(10));
            if complete_first {
                state.on_interrupt(&mut hw);
            }
            state.arm(10, Trigger::AnyEdge, &mut hw);
            assert!(!state.ready(10));
            assert_eq!(state.on_interrupt(&mut hw), 0);
            state.cancel(10, &mut hw);
            hw.change(10, !hw.high(10));
            assert_eq!(state.on_interrupt(&mut hw), 0);
        }
    }
}
