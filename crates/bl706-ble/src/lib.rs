//! Experimental on-chip HCI bridge for the BL70x peripheral controller.
#![no_std]

mod packet;
pub use packet::{Error, Packet};
#[cfg(all(feature = "controller", target_arch = "riscv32"))]
mod controller;
#[cfg(all(feature = "controller", target_arch = "riscv32"))]
mod platform;
#[cfg(all(feature = "controller", target_arch = "riscv32"))]
pub use controller::{NativeTransport, Runner, initialize, take_metrics};
