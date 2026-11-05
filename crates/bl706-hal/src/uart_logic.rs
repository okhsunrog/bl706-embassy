//! UART FIFO polling and one-shot IRQ ownership, independent of MMIO.
use core::task::{Context, Poll, Waker};

pub const TX_FIFO: u32 = 1 << 2;
pub const RX_FIFO: u32 = 1 << 3;
pub const PARITY: u32 = 1 << 5;
pub const TX_ERROR: u32 = 1 << 6;
pub const RX_ERROR: u32 = 1 << 7;
pub const TX_EVENTS: u32 = TX_FIFO | TX_ERROR;
pub const RX_EVENTS: u32 = RX_FIFO | PARITY | RX_ERROR;
pub const FIFO_SIZE: u8 = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidBaud,
    Timeout,
    Fifo,
    Overrun,
    Parity,
}

pub fn divider(clock: u32, baud: u32) -> Option<u16> {
    if baud == 0 {
        return None;
    }
    let divider = (u64::from(clock) + u64::from(baud) / 2) / u64::from(baud);
    (1..=65536).contains(&divider).then(|| (divider - 1) as u16)
}

pub trait Hardware {
    fn status(&mut self) -> u32;
    fn masked(&mut self) -> u32;
    fn mask(&mut self, events: u32);
    fn unmask(&mut self, events: u32);
    fn tx_space(&mut self) -> u8;
    fn rx_count(&mut self) -> u8;
    fn tx_threshold(&mut self, value: u8);
    fn write(&mut self, byte: u8);
    fn read(&mut self) -> u8;
    fn clear_tx(&mut self);
    fn clear_rx(&mut self);
}

pub struct State {
    tx: Option<Waker>,
    rx: Option<Waker>,
}
impl State {
    pub const fn new() -> Self {
        Self { tx: None, rx: None }
    }
    pub fn cancel(&mut self, hw: &mut impl Hardware, receive: bool) {
        hw.mask(if receive { RX_EVENTS } else { TX_EVENTS });
        if receive {
            self.rx = None;
        } else {
            self.tx = None;
        }
    }
    pub fn poll_read(
        &mut self,
        hw: &mut impl Hardware,
        cx: &Context<'_>,
        bytes: &mut [u8],
    ) -> Poll<Result<usize, Error>> {
        self.cancel(hw, true);
        if bytes.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let status = hw.status();
        if status & (PARITY | RX_ERROR) != 0 {
            hw.clear_rx();
            return Poll::Ready(Err(if status & RX_ERROR != 0 {
                Error::Overrun
            } else {
                Error::Parity
            }));
        }
        let count = bytes.len().min(usize::from(hw.rx_count()));
        if count != 0 {
            for byte in &mut bytes[..count] {
                *byte = hw.read();
            }
            return Poll::Ready(Ok(count));
        }
        self.rx = Some(cx.waker().clone());
        // FIFO/error status is level-sensitive: data arriving after the count
        // read remains pending when unmasked. Caller holds a critical section.
        hw.unmask(RX_EVENTS);
        Poll::Pending
    }
    pub fn poll_write(
        &mut self,
        hw: &mut impl Hardware,
        cx: &Context<'_>,
        bytes: &[u8],
    ) -> Poll<Result<usize, Error>> {
        self.cancel(hw, false);
        if bytes.is_empty() {
            return Poll::Ready(Ok(0));
        }
        if hw.status() & TX_ERROR != 0 {
            hw.clear_tx();
            return Poll::Ready(Err(Error::Fifo));
        }
        let count = bytes.len().min(usize::from(hw.tx_space()));
        if count != 0 {
            for &byte in &bytes[..count] {
                hw.write(byte);
            }
            return Poll::Ready(Ok(count));
        }
        hw.tx_threshold(0);
        self.tx = Some(cx.waker().clone());
        hw.unmask(TX_EVENTS);
        Poll::Pending
    }
    /// Wait for FIFO empty; the caller must still wait for TX bus idle.
    pub fn poll_drain(
        &mut self,
        hw: &mut impl Hardware,
        cx: &Context<'_>,
    ) -> Poll<Result<(), Error>> {
        self.cancel(hw, false);
        if hw.status() & TX_ERROR != 0 {
            hw.clear_tx();
            return Poll::Ready(Err(Error::Fifo));
        }
        if hw.tx_space() == FIFO_SIZE {
            return Poll::Ready(Ok(()));
        }
        hw.tx_threshold(FIFO_SIZE - 1);
        self.tx = Some(cx.waker().clone());
        hw.unmask(TX_EVENTS);
        Poll::Pending
    }
    pub fn on_interrupt(&mut self, hw: &mut impl Hardware) -> [Option<Waker>; 2] {
        let pending = hw.status() & !hw.masked();
        let tx = if pending & TX_EVENTS != 0 {
            hw.mask(TX_EVENTS);
            self.tx.take()
        } else {
            None
        };
        let rx = if pending & RX_EVENTS != 0 {
            hw.mask(RX_EVENTS);
            self.rx.take()
        } else {
            None
        };
        // Do not clear errors here: the task must observe them before data.
        [tx, rx]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::VecDeque,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        task::Wake,
    };

    struct Counter(AtomicUsize);
    impl Wake for Counter {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn waker() -> (Arc<Counter>, Waker) {
        let counter = Arc::new(Counter(AtomicUsize::new(0)));
        (counter.clone(), Waker::from(counter))
    }
    struct Fake {
        mask: u32,
        errors: u32,
        tx: Vec<u8>,
        rx: VecDeque<u8>,
        threshold: u8,
        arrival_on_unmask: Option<u8>,
    }
    impl Fake {
        fn new() -> Self {
            Self {
                mask: u32::MAX,
                errors: 0,
                tx: Vec::new(),
                rx: VecDeque::new(),
                threshold: 0,
                arrival_on_unmask: None,
            }
        }
    }
    impl Hardware for Fake {
        fn status(&mut self) -> u32 {
            self.errors
                | if self.tx_space() > self.threshold {
                    TX_FIFO
                } else {
                    0
                }
                | if self.rx.is_empty() { 0 } else { RX_FIFO }
        }
        fn masked(&mut self) -> u32 {
            self.mask
        }
        fn mask(&mut self, events: u32) {
            self.mask |= events;
        }
        fn unmask(&mut self, events: u32) {
            if events & RX_FIFO != 0 {
                if let Some(byte) = self.arrival_on_unmask.take() {
                    self.rx.push_back(byte);
                }
            }
            self.mask &= !events;
        }
        fn tx_space(&mut self) -> u8 {
            FIFO_SIZE - self.tx.len() as u8
        }
        fn rx_count(&mut self) -> u8 {
            self.rx.len() as u8
        }
        fn tx_threshold(&mut self, value: u8) {
            self.threshold = value;
        }
        fn write(&mut self, byte: u8) {
            assert!(self.tx.len() < usize::from(FIFO_SIZE));
            self.tx.push(byte);
        }
        fn read(&mut self) -> u8 {
            self.rx.pop_front().unwrap()
        }
        fn clear_tx(&mut self) {
            self.tx.clear();
            self.errors &= !TX_ERROR;
        }
        fn clear_rx(&mut self) {
            self.rx.clear();
            self.errors &= !(PARITY | RX_ERROR);
        }
    }
    fn wake_all(wakers: [Option<Waker>; 2]) {
        for w in wakers.into_iter().flatten() {
            w.wake();
        }
    }
    #[test]
    fn baud_boundaries_and_overflow() {
        assert_eq!(divider(32_000_000, 115_200), Some(277));
        assert_eq!(divider(65_536, 1), Some(65_535));
        assert_eq!(divider(65_537, 1), None);
        assert_eq!(divider(32_000_000, 0), None);
        assert_eq!(divider(32_000_000, u32::MAX), None);
        assert_eq!(divider(u32::MAX, u32::MAX), Some(0));
    }
    #[test]
    fn data_before_poll_and_partial_read() {
        let mut state = State::new();
        let mut hw = Fake::new();
        hw.rx.extend([1, 2, 3]);
        let (_, w) = waker();
        let mut out = [0; 2];
        assert_eq!(
            state.poll_read(&mut hw, &Context::from_waker(&w), &mut out),
            Poll::Ready(Ok(2))
        );
        assert_eq!(out, [1, 2]);
        assert_eq!(hw.rx, [3]);
        assert_eq!(hw.mask & RX_EVENTS, RX_EVENTS);
    }
    #[test]
    fn arrival_during_arm_is_not_lost_and_irq_is_one_shot() {
        let mut state = State::new();
        let mut hw = Fake::new();
        hw.arrival_on_unmask = Some(42);
        let (counter, w) = waker();
        let mut out = [0];
        assert!(
            state
                .poll_read(&mut hw, &Context::from_waker(&w), &mut out)
                .is_pending()
        );
        wake_all(state.on_interrupt(&mut hw));
        wake_all(state.on_interrupt(&mut hw));
        assert_eq!(counter.0.load(Ordering::SeqCst), 1);
        assert_eq!(
            state.poll_read(&mut hw, &Context::from_waker(&w), &mut out),
            Poll::Ready(Ok(1))
        );
        assert_eq!(out, [42]);
    }
    #[test]
    fn write_larger_than_fifo_then_irq_and_resume() {
        let mut state = State::new();
        let mut hw = Fake::new();
        let (counter, w) = waker();
        let cx = Context::from_waker(&w);
        let bytes: Vec<_> = (0..=255).collect();
        assert_eq!(state.poll_write(&mut hw, &cx, &bytes), Poll::Ready(Ok(128)));
        assert_eq!(hw.tx, bytes[..128]);
        assert!(state.poll_write(&mut hw, &cx, &bytes[128..]).is_pending());
        let sent = std::mem::take(&mut hw.tx);
        wake_all(state.on_interrupt(&mut hw));
        assert_eq!(counter.0.load(Ordering::SeqCst), 1);
        assert_eq!(
            state.poll_write(&mut hw, &cx, &bytes[128..]),
            Poll::Ready(Ok(128))
        );
        assert_eq!([sent, hw.tx].concat(), bytes);
    }
    #[test]
    fn cancel_rx_keeps_tx_waiter_and_received_bytes() {
        let mut state = State::new();
        let mut hw = Fake::new();
        hw.tx.resize(128, 0);
        let (tx_counter, tx) = waker();
        let (rx_counter, rx) = waker();
        assert!(
            state
                .poll_write(&mut hw, &Context::from_waker(&tx), &[8])
                .is_pending()
        );
        assert!(
            state
                .poll_read(&mut hw, &Context::from_waker(&rx), &mut [0])
                .is_pending()
        );
        state.cancel(&mut hw, true);
        hw.rx.push_back(9);
        hw.tx.clear();
        wake_all(state.on_interrupt(&mut hw));
        assert_eq!(tx_counter.0.load(Ordering::SeqCst), 1);
        assert_eq!(rx_counter.0.load(Ordering::SeqCst), 0);
        let mut out = [0];
        assert_eq!(
            state.poll_read(&mut hw, &Context::from_waker(&rx), &mut out),
            Poll::Ready(Ok(1))
        );
        assert_eq!(out, [9]);
    }
    #[test]
    fn cancel_tx_keeps_rx_waiter_and_queued_bytes() {
        let mut state = State::new();
        let mut hw = Fake::new();
        hw.tx.resize(128, 7);
        let (counter, w) = waker();
        let cx = Context::from_waker(&w);
        assert!(state.poll_write(&mut hw, &cx, &[1]).is_pending());
        assert!(state.poll_read(&mut hw, &cx, &mut [0]).is_pending());
        state.cancel(&mut hw, false);
        assert_eq!(hw.tx, [7; 128]);
        hw.rx.push_back(1);
        wake_all(state.on_interrupt(&mut hw));
        assert_eq!(counter.0.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn errors_latched_until_task_then_clear_and_reuse() {
        for error in [PARITY, RX_ERROR, PARITY | RX_ERROR] {
            let mut state = State::new();
            let mut hw = Fake::new();
            let (_, w) = waker();
            let cx = Context::from_waker(&w);
            let mut out = [99];
            assert!(state.poll_read(&mut hw, &cx, &mut out).is_pending());
            hw.rx.push_back(7);
            hw.errors = error;
            wake_all(state.on_interrupt(&mut hw));
            assert_eq!(hw.errors, error);
            assert_eq!(
                state.poll_read(&mut hw, &cx, &mut out),
                Poll::Ready(Err(if error & RX_ERROR != 0 {
                    Error::Overrun
                } else {
                    Error::Parity
                }))
            );
            assert_eq!(out, [99]);
            assert_eq!(hw.errors, 0);
            hw.rx.push_back(8);
            assert_eq!(state.poll_read(&mut hw, &cx, &mut out), Poll::Ready(Ok(1)));
            assert_eq!(out, [8]);
        }
    }
    #[test]
    fn flush_waits_until_fifo_empty_and_write_restores_threshold() {
        let mut state = State::new();
        let mut hw = Fake::new();
        hw.tx.push(1);
        let (_, w) = waker();
        let cx = Context::from_waker(&w);
        assert!(state.poll_drain(&mut hw, &cx).is_pending());
        assert_eq!(hw.threshold, 127);
        assert!(state.on_interrupt(&mut hw).iter().all(Option::is_none));
        hw.tx.clear();
        wake_all(state.on_interrupt(&mut hw));
        assert_eq!(state.poll_drain(&mut hw, &cx), Poll::Ready(Ok(())));
        hw.tx.resize(128, 0);
        assert!(state.poll_write(&mut hw, &cx, &[1]).is_pending());
        assert_eq!(hw.threshold, 0);
    }
    #[test]
    fn empty_buffers_do_not_arm_or_touch_fifo() {
        let mut state = State::new();
        let mut hw = Fake::new();
        let (_, w) = waker();
        let cx = Context::from_waker(&w);
        hw.rx.push_back(1);
        assert_eq!(state.poll_read(&mut hw, &cx, &mut []), Poll::Ready(Ok(0)));
        assert_eq!(state.poll_write(&mut hw, &cx, &[]), Poll::Ready(Ok(0)));
        assert_eq!(hw.rx.len(), 1);
        assert_eq!(hw.mask, u32::MAX);
    }
    #[test]
    fn tx_error_is_reported_for_write_and_flush() {
        let mut state = State::new();
        let mut hw = Fake::new();
        let (_, w) = waker();
        let cx = Context::from_waker(&w);
        hw.errors = TX_ERROR;
        assert_eq!(
            state.poll_write(&mut hw, &cx, &[1]),
            Poll::Ready(Err(Error::Fifo))
        );
        hw.errors = TX_ERROR;
        assert_eq!(
            state.poll_drain(&mut hw, &cx),
            Poll::Ready(Err(Error::Fifo))
        );
        assert_eq!(state.poll_write(&mut hw, &cx, &[1]), Poll::Ready(Ok(1)));
    }
}
