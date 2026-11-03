//! Polling UART0 TX for bring-up logs, Pinecil GPIO22. RX is not enabled.
use crate::{
    clock::Clocks,
    gpio::{Pin, Pull},
    pac,
};
use core::fmt;
pub struct Uart0Token {
    _private: (),
}
impl Uart0Token {
    pub(crate) unsafe fn new() -> Self {
        Self { _private: () }
    }
}
pub struct Uart0 {
    _token: Uart0Token,
    _tx: Pin<22>,
}
#[derive(Debug)]
pub enum Error {
    InvalidBaud,
    Timeout,
}

impl Uart0 {
    pub fn new(token: Uart0Token, tx: Pin<22>, clocks: Clocks, baud: u32) -> Result<Self, Error> {
        if baud == 0 {
            return Err(Error::InvalidBaud);
        }
        let divider = (clocks.uart_hz() + baud / 2) / baud;
        if !(1..=65536).contains(&divider) {
            return Err(Error::InvalidBaud);
        }
        tx.configure(7, Pull::None, true);
        pac::GLB.glb_parm().modify(|r| r.set_uart_swap_set(0));
        // Signal index = GPIO mod 8, UART0_TXD=2. Keep other signals intact.
        pac::GLB
            .UART_SIG_SEL_0()
            .modify(|r| r.0 = (r.0 & !(0xf << 24)) | (2 << 24));
        pac::UART0.utx_config().write(|_| {});
        pac::UART0.urx_config().write(|_| {});
        pac::UART0.uart_int_mask().write(|r| r.0 = 0xff);
        pac::UART0.uart_int_en().write(|_| {});
        pac::UART0.uart_sw_mode().write(|_| {});
        pac::UART0.data_config().write(|_| {});
        pac::UART0
            .uart_fifo_config_0()
            .write(|r| r.0 = (1 << 2) | (1 << 3));
        pac::UART0
            .uart_bit_prd()
            .write(|r| r.0 = (divider - 1) * 0x0001_0001);
        // 8 data bits, one stop bit (encoded in half-bit units), free-run.
        pac::UART0.utx_config().write(|r| {
            r.set_cr_utx_bit_cnt_d(7);
            r.set_cr_utx_bit_cnt_p(1);
            r.set_cr_utx_frm_en(true);
            r.set_cr_utx_en(true);
        });
        Ok(Self {
            _token: token,
            _tx: tx,
        })
    }
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        for &byte in bytes {
            let start = crate::time::now_ticks();
            while pac::UART0.uart_fifo_config_1().read().tx_fifo_cnt() == 0 {
                if crate::time::now_ticks().wrapping_sub(start) > 100_000 {
                    return Err(Error::Timeout);
                }
                core::hint::spin_loop();
            }
            pac::UART0
                .uart_fifo_wdata()
                .write(|r| r.set_uart_fifo_wdata(byte));
        }
        Ok(())
    }
}
impl fmt::Write for Uart0 {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write(s.as_bytes()).map_err(|_| fmt::Error)
    }
}
