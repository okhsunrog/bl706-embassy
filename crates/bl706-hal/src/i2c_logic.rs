//! Owned packet state shared by the task and ISR under a critical section.
//! No caller buffer references or pointers escape into interrupt state.

pub const MAX_PACKET: usize = 256;
pub const END: u32 = 1 << 0;
pub const TX_READY: u32 = 1 << 1;
pub const RX_READY: u32 = 1 << 2;
pub const NACK: u32 = 1 << 3;
pub const ARBITRATION: u32 = 1 << 4;
pub const FIFO_ERROR: u32 = 1 << 5;
const ERRORS: u32 = NACK | ARBITRATION | FIFO_ERROR;

pub fn pack(bytes: &[u8]) -> u32 {
    assert!(bytes.len() <= 4);
    bytes
        .iter()
        .enumerate()
        .fold(0, |word, (i, b)| word | (u32::from(*b) << (8 * i)))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferError {
    NoAcknowledge,
    ArbitrationLost,
    Fifo,
    ShortTransfer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Direction {
    Read,
    Write,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    Active,
    Complete(Result<(), TransferError>),
}

/// Only the register backend touches hardware. Tests use a two-word FIFO model.
pub trait Hardware {
    fn status(&mut self) -> u32;
    fn tx_space(&mut self) -> u8;
    fn rx_words(&mut self) -> u8;
    fn write_word(&mut self, word: u32);
    fn read_word(&mut self) -> u32;
    fn listen(&mut self, events: u32);
    /// Disable the IRQ source and controller, clear status and both FIFOs.
    fn stop(&mut self);
}

pub struct Transfer {
    bytes: [u8; MAX_PACKET],
    len: usize,
    done: usize,
    direction: Direction,
    phase: Phase,
}

impl Transfer {
    pub const fn new() -> Self {
        Self {
            bytes: [0; MAX_PACKET],
            len: 0,
            done: 0,
            direction: Direction::Read,
            phase: Phase::Idle,
        }
    }

    pub fn is_active(&self) -> bool {
        self.phase == Phase::Active
    }

    fn begin(&mut self, len: usize, direction: Direction) {
        assert!(!self.is_active());
        assert!((1..=MAX_PACKET).contains(&len));
        self.len = len;
        self.done = 0;
        self.direction = direction;
        self.phase = Phase::Active;
    }

    pub fn begin_read(&mut self, len: usize) {
        self.begin(len, Direction::Read);
    }

    pub fn begin_write(&mut self, bytes: &[u8]) {
        self.begin(bytes.len(), Direction::Write);
        self.bytes[..bytes.len()].copy_from_slice(bytes);
    }

    /// Preload TX before starting the controller. RX is drained only in the ISR.
    pub fn prime(&mut self, hw: &mut impl Hardware) {
        assert!(self.is_active());
        if self.direction == Direction::Write {
            self.pump(hw);
        }
        hw.listen(self.events());
    }

    fn events(&self) -> u32 {
        let fifo = if self.done == self.len {
            0
        } else if self.direction == Direction::Read {
            RX_READY
        } else {
            TX_READY
        };
        END | ERRORS | fifo
    }

    /// Bound the ISR to two FIFO word accesses, even if hardware keeps advancing.
    fn pump(&mut self, hw: &mut impl Hardware) {
        for _ in 0..2 {
            if self.done == self.len {
                break;
            }
            let n = core::cmp::min(4, self.len - self.done);
            match self.direction {
                Direction::Write => {
                    if hw.tx_space() == 0 {
                        break;
                    }
                    hw.write_word(pack(&self.bytes[self.done..self.done + n]));
                }
                Direction::Read => {
                    if hw.rx_words() == 0 {
                        break;
                    }
                    let word = hw.read_word().to_le_bytes();
                    self.bytes[self.done..self.done + n].copy_from_slice(&word[..n]);
                }
            }
            self.done += n;
        }
    }

    fn error(status: u32) -> Option<TransferError> {
        if status & NACK != 0 {
            Some(TransferError::NoAcknowledge)
        } else if status & ARBITRATION != 0 {
            Some(TransferError::ArbitrationLost)
        } else if status & FIFO_ERROR != 0 {
            Some(TransferError::Fifo)
        } else {
            None
        }
    }

    fn complete(&mut self, hw: &mut impl Hardware, result: Result<(), TransferError>) -> bool {
        // Quiesce hardware before publishing completion to the task. Never
        // clear the owned packet here: the successful RX still needs copying.
        hw.stop();
        self.phase = Phase::Complete(result);
        true
    }

    /// Service a FIFO interrupt without polling the future. Returns true only
    /// when a new terminal result has been latched and the waiter must be woken.
    pub fn on_interrupt(&mut self, hw: &mut impl Hardware) -> bool {
        if !self.is_active() {
            hw.stop(); // stale pending IRQ after completion/cancel
            return false;
        }
        let before = hw.status();
        if let Some(error) = Self::error(before) {
            return self.complete(hw, Err(error));
        }
        // Drain final RX data on END; do not refill a TX which has already ended.
        if self.direction == Direction::Read || before & END == 0 {
            self.pump(hw);
        }
        let after = hw.status();
        if let Some(error) = Self::error(after) {
            return self.complete(hw, Err(error));
        }
        if before & END != 0 || (after & END != 0 && self.done == self.len) {
            let result = if self.done == self.len {
                Ok(())
            } else {
                Err(TransferError::ShortTransfer)
            };
            return self.complete(hw, result);
        }
        // END may have arrived after pump saw an empty RX FIFO, along with the
        // final word. Leave END armed so the next bounded ISR drains that word.
        hw.listen(self.events());
        false
    }

    pub fn result(&self) -> Option<Result<(), TransferError>> {
        match self.phase {
            Phase::Complete(result) => Some(result),
            _ => None,
        }
    }

    pub fn copy_read(&self, out: &mut [u8]) {
        assert_eq!(self.phase, Phase::Complete(Ok(())));
        assert_eq!(self.direction, Direction::Read);
        assert_eq!(out.len(), self.len);
        out.copy_from_slice(&self.bytes[..self.len]);
    }

    pub fn cancel(&mut self, hw: &mut impl Hardware) {
        hw.stop();
        self.phase = Phase::Idle;
        self.len = 0;
        self.done = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[derive(Default)]
    struct Model {
        status: u32,
        tx: VecDeque<u32>,
        rx: VecDeque<u32>,
        mask: u32,
        stopped: bool,
        reads: usize,
        writes: usize,
        stops: usize,
        status_script: VecDeque<(u32, Option<u32>)>,
    }
    impl Hardware for Model {
        fn status(&mut self) -> u32 {
            if let Some((status, rx_word)) = self.status_script.pop_front() {
                self.status = status;
                if let Some(word) = rx_word {
                    self.rx.push_back(word);
                }
            }
            self.status
        }
        fn tx_space(&mut self) -> u8 {
            (2 - self.tx.len()) as u8
        }
        fn rx_words(&mut self) -> u8 {
            self.rx.len() as u8
        }
        fn write_word(&mut self, word: u32) {
            assert!(self.tx.len() < 2, "TX overflow");
            self.writes += 1;
            self.tx.push_back(word);
        }
        fn read_word(&mut self) -> u32 {
            self.reads += 1;
            self.rx.pop_front().expect("RX underflow")
        }
        fn listen(&mut self, events: u32) {
            self.mask = events;
            self.stopped = false;
        }
        fn stop(&mut self) {
            self.stops += 1;
            self.stopped = true;
            self.mask = 0;
            self.status = 0;
            self.tx.clear();
            self.rx.clear();
        }
    }

    #[test]
    fn sub_address_and_partial_word_order() {
        assert_eq!(pack(&[0x12, 0x34]), 0x3412);
        assert_eq!(pack(&[0xaa, 0xbb, 0xcc]), 0x00ccbbaa);
        assert_eq!(pack(&[]), 0);
    }

    #[test]
    fn tx_all_lengths_advance_in_isr_without_future_polling() {
        for len in 1..=256 {
            let input: Vec<u8> = (0..len).map(|n| n as u8).collect();
            let mut transfer = Transfer::new();
            let mut hw = Model::default();
            transfer.begin_write(&input);
            transfer.prime(&mut hw);
            let mut wire = Vec::new();
            while wire.len() < len {
                let word = hw
                    .tx
                    .pop_front()
                    .expect("TX starved before END")
                    .to_le_bytes();
                let n = (len - wire.len()).min(4);
                wire.extend_from_slice(&word[..n]);
                assert!(word[n..].iter().all(|b| *b == 0));
                hw.status = if wire.len() == len { END } else { TX_READY };
                let writes = hw.writes;
                let wake = transfer.on_interrupt(&mut hw);
                assert!(hw.writes - writes <= 2);
                assert_eq!(wake, wire.len() == len);
                if !wake {
                    assert_eq!(transfer.result(), None);
                }
            }
            assert_eq!(wire, input);
            assert_eq!(transfer.result(), Some(Ok(())));
            assert!(hw.stopped);
            assert_eq!(hw.mask, 0);
            assert!(!transfer.on_interrupt(&mut hw), "duplicate completion wake");
        }
    }

    #[test]
    fn rx_all_lengths_advance_in_isr_and_copy_only_after_end() {
        for len in 1..=256 {
            let input: Vec<u8> = (0..len).map(|n| n as u8).collect();
            let mut transfer = Transfer::new();
            let mut hw = Model::default();
            transfer.begin_read(len);
            transfer.prime(&mut hw);
            let mut received = 0;
            for batch in input.chunks(8) {
                for chunk in batch.chunks(4) {
                    let mut padded = [0xee; 4];
                    padded[..chunk.len()].copy_from_slice(chunk);
                    hw.rx.push_back(u32::from_le_bytes(padded));
                }
                received += batch.len();
                hw.status = RX_READY | if received == len { END } else { 0 };
                let reads = hw.reads;
                let wake = transfer.on_interrupt(&mut hw);
                assert!(hw.reads - reads <= 2);
                assert_eq!(wake, received == len);
                if !wake {
                    assert_eq!(transfer.result(), None);
                }
            }
            let mut output = vec![0xcc; len];
            transfer.copy_read(&mut output);
            assert_eq!(output, input);
            assert!(hw.stopped);
            assert_eq!(hw.mask, 0);
        }
    }

    #[test]
    fn full_tx_and_empty_rx_do_not_touch_fifo_or_complete() {
        let mut transfer = Transfer::new();
        let mut hw = Model::default();
        transfer.begin_write(&[1; 9]);
        transfer.prime(&mut hw);
        assert_eq!(transfer.done, 8);
        assert!(!transfer.on_interrupt(&mut hw));
        assert_eq!(hw.writes, 2);
        assert_eq!(transfer.done, 8);
        transfer.cancel(&mut hw);
        transfer.begin_read(9);
        transfer.prime(&mut hw);
        assert!(!transfer.on_interrupt(&mut hw));
        assert_eq!(hw.reads, 0);
        assert_eq!(transfer.done, 0);
    }

    #[test]
    fn errors_win_over_end_and_quiesce_before_return() {
        for (flag, error) in [
            (NACK, TransferError::NoAcknowledge),
            (ARBITRATION, TransferError::ArbitrationLost),
            (FIFO_ERROR, TransferError::Fifo),
        ] {
            for late in [false, true] {
                let mut transfer = Transfer::new();
                let mut hw = Model::default();
                transfer.begin_read(1);
                transfer.prime(&mut hw);
                hw.rx.push_back(0x42);
                hw.status = END | flag;
                if late {
                    hw.status_script = [(0, None), (END | flag, None)].into();
                }
                assert!(transfer.on_interrupt(&mut hw));
                assert_eq!(transfer.result(), Some(Err(error)));
                assert!(hw.stopped);
                assert_eq!(hw.mask, 0);
                assert_eq!(hw.reads, usize::from(late));
            }
        }
    }

    #[test]
    fn premature_end_is_not_success_and_never_refills_tx() {
        let mut transfer = Transfer::new();
        let mut hw = Model::default();
        transfer.begin_write(&[1; 9]);
        transfer.prime(&mut hw);
        hw.tx.clear();
        hw.status = END;
        assert!(transfer.on_interrupt(&mut hw));
        assert_eq!(transfer.result(), Some(Err(TransferError::ShortTransfer)));
        assert_eq!(hw.writes, 2);
        transfer.cancel(&mut hw);
        transfer.begin_read(9);
        transfer.prime(&mut hw);
        hw.rx = [0x04030201, 0x08070605].into();
        hw.status = END;
        assert!(transfer.on_interrupt(&mut hw));
        assert_eq!(transfer.result(), Some(Err(TransferError::ShortTransfer)));
    }

    #[test]
    fn late_end_with_final_rx_word_is_drained_on_next_irq() {
        let mut transfer = Transfer::new();
        let mut hw = Model::default();
        transfer.begin_read(1);
        transfer.prime(&mut hw);
        hw.status_script = [(0, None), (END, Some(0x42))].into();
        assert!(!transfer.on_interrupt(&mut hw));
        assert_eq!(transfer.result(), None);
        assert_ne!(hw.mask & END, 0);
        assert!(transfer.on_interrupt(&mut hw));
        assert_eq!(transfer.result(), Some(Ok(())));
        let mut output = [0];
        transfer.copy_read(&mut output);
        assert_eq!(output, [0x42]);
    }

    #[test]
    fn cancellation_and_stale_irq_allow_reusing_owned_packet() {
        for read in [false, true] {
            let mut transfer = Transfer::new();
            let mut hw = Model::default();
            if read {
                transfer.begin_read(256);
            } else {
                transfer.begin_write(&[0x55; 256]);
            }
            transfer.prime(&mut hw);
            transfer.cancel(&mut hw); // same engine path as timeout / future Drop
            assert!(!transfer.is_active());
            assert_eq!(transfer.result(), None);
            assert!(hw.stopped);
            assert_eq!(hw.mask, 0);
            hw.status = END | RX_READY | TX_READY;
            hw.rx.push_back(0xdeadbeef);
            let counts = (hw.reads, hw.writes);
            assert!(!transfer.on_interrupt(&mut hw));
            assert_eq!((hw.reads, hw.writes), counts);
            transfer.begin_read(1);
            transfer.prime(&mut hw);
            hw.rx.push_back(0x42);
            hw.status = END;
            assert!(transfer.on_interrupt(&mut hw));
            let mut output = [0];
            transfer.copy_read(&mut output);
            assert_eq!(output, [0x42]);
        }
    }

    #[test]
    fn staged_tx_is_independent_of_the_callers_storage() {
        let mut input = [0x55; 12];
        let mut transfer = Transfer::new();
        let mut hw = Model::default();
        transfer.begin_write(&input);
        input.fill(0xcc); // Even a forgotten future shares no caller memory with ISR.
        transfer.prime(&mut hw);
        assert_eq!(hw.tx.pop_front(), Some(0x55555555));
        assert_eq!(hw.tx.pop_front(), Some(0x55555555));
        hw.status = TX_READY;
        assert!(!transfer.on_interrupt(&mut hw));
        assert_eq!(hw.tx.pop_front(), Some(0x55555555));
    }

    #[test]
    fn fifo_progress_and_all_bytes_queued_do_not_wake_until_end() {
        let mut transfer = Transfer::new();
        let mut hw = Model::default();
        transfer.begin_write(&[0x55; 12]);
        transfer.prime(&mut hw);
        hw.tx.clear();
        hw.status = TX_READY;
        assert!(!transfer.on_interrupt(&mut hw));
        assert_eq!(transfer.done, 12);
        assert_eq!(transfer.result(), None);
        assert_eq!(hw.mask & TX_READY, 0);
        assert_ne!(hw.mask & END, 0);
        hw.status = END;
        assert!(transfer.on_interrupt(&mut hw));
        assert_eq!(transfer.result(), Some(Ok(())));
    }
}
