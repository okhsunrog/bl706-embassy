//! ROM-backed access to a reserved region while the rest of firmware uses XIP.
use super::{
    Chip, Error, FlashToken,
    logic::{PAGE, SECTOR, Window},
};
use core::ops::Range;

const ROM_TABLE: usize = 0x2101_8800;
const ROM_GET_ID: usize = 81;
const ROM_WRITE: usize = 79;
const ROM_ERASE: usize = 78;
const ROM_FLUSH: usize = 121;
const FLASH_CAPACITY: u32 = 1024 * 1024;
const IMAGE_OFFSET: u32 = 0x1000;

#[repr(C, align(4))]
struct FlashConfig([u8; 84]);

// The reviewed MX25L8006 dual-output boot profile. ioMode bit 4 disables wrap.
// Same byte layout as SDK SPI_Flash_Cfg_Type, excluding the FCFG magic.
const MX25L8006: FlashConfig = FlashConfig([
    0x11, 0x00, 0x01, 0x01, 0x66, 0x99, 0xff, 0x03, 0x9f, 0x00, 0x9f, 0x00, 0x04, 0xc2, 0x00, 0x01,
    0xc7, 0x20, 0x52, 0xd8, 0x06, 0x02, 0x32, 0x00, 0x0b, 0x01, 0x0b, 0x01, 0x3b, 0x01, 0xbb, 0x00,
    0x6b, 0x01, 0xeb, 0x02, 0xeb, 0x02, 0x02, 0x50, 0x00, 0x01, 0x00, 0x01, 0x01, 0x00, 0x02, 0x01,
    0x02, 0x01, 0xab, 0x01, 0x05, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x38, 0xff, 0x00, 0xff,
    0x77, 0x03, 0x02, 0x40, 0x77, 0x03, 0x02, 0xf0, 0x2c, 0x01, 0xb0, 0x04, 0xb0, 0x04, 0x05, 0x00,
    0xff, 0xff, 0x14, 0x00,
]);

/// A single-owner, region-relative NOR handle for the validated MX25L8006.
/// ROM operations mask interrupts until flash is idle and XIP is restored.
/// The async interface yields between pages/sectors, not during their busy time.
pub struct XipFlash {
    _token: FlashToken,
    window: Window,
    physical_start: u32,
    config: FlashConfig,
    functions: [usize; 4],
    erase_allowed: bool,
    metrics: Metrics,
}

/// Physical operations and their longest blocking duration, in microseconds.
#[derive(Clone, Copy, Debug, Default)]
pub struct Metrics {
    pub programs: u32,
    pub erases: u32,
    pub max_program_us: u32,
    pub max_erase_us: u32,
}

impl XipFlash {
    /// Use the ROM XIP helpers with the MX25L8006 dual-output profile.
    ///
    /// # Safety
    /// The erase-aligned region must be reserved outside firmware/factory data.
    /// There must be no concurrent flash controller, DMA or debugger accesses
    /// during calls. The trampoline must be linked into copied executable RAM.
    /// Firmware must tolerate masked interrupts (up to a sector erase's maximum
    /// duration); callers must arrange safe actuator/radio/protocol states.
    pub unsafe fn new_mx25l8006(token: FlashToken, region: Range<u32>) -> Result<Self, Error> {
        let ram_entry = transaction as *const () as usize;
        if !(0x2201_0000..0x2202_c000).contains(&ram_entry)
            || region.start < IMAGE_OFFSET
            || region.end > FLASH_CAPACITY
            || crate::pac::SF_CTRL.sf_id0_offset().read().0 != IMAGE_OFFSET
        {
            return Err(Error::OutOfBounds);
        }
        let mut functions = [0; 4];
        for (out, index) in functions
            .iter_mut()
            .zip([ROM_GET_ID, ROM_WRITE, ROM_ERASE, ROM_FLUSH])
        {
            *out = unsafe { (ROM_TABLE as *const u32).add(index).read_volatile() } as usize;
            if !(0x2100_0000..0x2102_0000).contains(out) || *out & 1 != 0 {
                return Err(Error::UnsupportedChip);
            }
        }
        let chip = Chip::identify([0xc2, 0x20, 0x14])?;
        let start = region.start;
        let window = Window::new(chip, chip.jedec_id, region)?;
        let mut flash = Self {
            _token: token,
            window,
            physical_start: start,
            config: MX25L8006,
            functions,
            erase_allowed: false,
            metrics: Metrics::default(),
        };
        let mut id = [0u8; 4];
        unsafe {
            transaction(
                &mut flash.config,
                flash.functions.as_ptr(),
                0,
                0,
                id.as_mut_ptr(),
                4,
            );
        }
        if id[..3] != chip.jedec_id {
            return Err(Error::WrongDevice);
        }
        Ok(flash)
    }

    /// Permit physical sector erases only when the application can tolerate
    /// their entire interrupt blackout. Blank sectors never require an erase.
    /// Disabled by default; a denied erase returns `Error::Busy`.
    pub fn set_erase_allowed(&mut self, allowed: bool) {
        self.erase_allowed = allowed;
    }

    pub fn metrics(&self) -> Metrics {
        self.metrics
    }

    pub fn capacity(&self) -> usize {
        self.window.capacity()
    }

    pub fn read(&mut self, offset: u32, data: &mut [u8]) -> Result<(), Error> {
        let physical = self.window.map(offset, data.len())?;
        let address = 0x2300_0000 + physical - IMAGE_OFFSET;
        for (index, value) in data.iter_mut().enumerate() {
            *value = unsafe { (address as *const u8).add(index).read_volatile() };
        }
        Ok(())
    }

    pub fn write(&mut self, offset: u32, data: &[u8]) -> Result<(), Error> {
        self.window.map(offset, data.len())?;
        let mut done = 0;
        while done < data.len() {
            let position = self.physical_start + offset + done as u32;
            let len = super::logic::page_chunk(position, data.len() - done);
            // A source may itself be in XIP. Copy it before disabling XIP.
            let mut buffer = [0u8; PAGE];
            buffer[..len].copy_from_slice(&data[done..done + len]);
            let mut old = [0u8; PAGE];
            self.read(offset + done as u32, &mut old[..len])?;
            if old[..len]
                .iter()
                .zip(&buffer[..len])
                .any(|(a, b)| a & b != *b)
            {
                return Err(Error::NeedsErase);
            }
            let start = crate::time::now_ticks();
            unsafe {
                transaction(
                    &mut self.config,
                    self.functions.as_ptr(),
                    1,
                    position,
                    buffer.as_mut_ptr(),
                    len as u32,
                );
            }
            self.metrics.programs += 1;
            self.metrics.max_program_us = self
                .metrics
                .max_program_us
                .max((crate::time::now_ticks() - start) as u32);
            self.read(offset + done as u32, &mut old[..len])?;
            if old[..len] != buffer[..len] {
                return Err(Error::Verify);
            }
            done += len;
        }
        Ok(())
    }

    pub fn erase(&mut self, from: u32, to: u32) -> Result<(), Error> {
        self.window.erase_range(from, to)?;
        let mut buffer = [0u8; PAGE];
        for offset in (from..to).step_by(SECTOR as usize) {
            let mut blank = true;
            for page in (offset..offset + SECTOR).step_by(PAGE) {
                self.read(page, &mut buffer)?;
                blank &= buffer.iter().all(|v| *v == 255);
            }
            if blank {
                continue;
            }
            if !self.erase_allowed {
                return Err(Error::Busy);
            }
            let start = crate::time::now_ticks();
            unsafe {
                transaction(
                    &mut self.config,
                    self.functions.as_ptr(),
                    2,
                    self.physical_start + offset,
                    core::ptr::null_mut(),
                    SECTOR,
                );
            }
            self.metrics.erases += 1;
            self.metrics.max_erase_us = self
                .metrics
                .max_erase_us
                .max((crate::time::now_ticks() - start) as u32);
            for page in (offset..offset + SECTOR).step_by(PAGE) {
                self.read(page, &mut buffer)?;
                if buffer.iter().any(|v| *v != 255) {
                    return Err(Error::Verify);
                }
            }
        }
        Ok(())
    }
}

/// The complete blackout path: this function and all its callees are RAM/ROM.
/// No formatting, logging, panic paths, flash constants or async suspension.
#[unsafe(export_name = "bl706_xip_flash_transaction")]
#[unsafe(link_section = ".text.flash_transaction")]
#[inline(never)]
unsafe extern "C" fn transaction(
    config: *mut FlashConfig,
    functions: *const usize,
    op: u32,
    address: u32,
    data: *mut u8,
    len: u32,
) {
    let old_mstatus: u32;
    unsafe {
        core::arch::asm!("csrrci {}, mstatus, 8", out(reg) old_mstatus);
        core::arch::asm!("fence iorw,iorw");
        let result = if op == 0 {
            let f: unsafe extern "C" fn(*mut FlashConfig, u32, *mut u8) -> u32 =
                core::mem::transmute(functions.read());
            f(config, 1, data)
        } else if op == 1 {
            let f: unsafe extern "C" fn(*mut FlashConfig, u32, u32, *mut u8, u32) -> u32 =
                core::mem::transmute(functions.add(1).read());
            f(config, 1, address, data, len)
        } else {
            let f: unsafe extern "C" fn(*mut FlashConfig, u32, u32, u32) -> u32 =
                core::mem::transmute(functions.add(2).read());
            f(config, 1, address, address + len - 1)
        };
        // A ROM timeout cannot authorize execution from a potentially busy chip.
        // Reset from RAM rather than returning into an uncertain XIP mapping.
        if result != 0 {
            crate::reset::system_reset();
        }
        let flush: unsafe extern "C" fn(u8) -> u32 = core::mem::transmute(functions.add(3).read());
        if flush(0xff) != 0 {
            crate::reset::system_reset();
        }
        core::arch::asm!("fence iorw,iorw", "fence.i");
        if old_mstatus & 8 != 0 {
            core::arch::asm!("csrsi mstatus,8");
        }
    }
}

impl embedded_storage::nor_flash::ErrorType for XipFlash {
    type Error = Error;
}
impl embedded_storage::nor_flash::ReadNorFlash for XipFlash {
    const READ_SIZE: usize = 1;
    fn capacity(&self) -> usize {
        self.capacity()
    }
    fn read(&mut self, offset: u32, data: &mut [u8]) -> Result<(), Error> {
        self.read(offset, data)
    }
}
impl embedded_storage::nor_flash::NorFlash for XipFlash {
    const WRITE_SIZE: usize = 1;
    const ERASE_SIZE: usize = SECTOR as usize;
    fn write(&mut self, offset: u32, data: &[u8]) -> Result<(), Error> {
        self.write(offset, data)
    }
    fn erase(&mut self, from: u32, to: u32) -> Result<(), Error> {
        self.erase(from, to)
    }
}
impl embedded_storage_async::nor_flash::ReadNorFlash for XipFlash {
    const READ_SIZE: usize = 1;
    fn capacity(&self) -> usize {
        self.capacity()
    }
    async fn read(&mut self, offset: u32, data: &mut [u8]) -> Result<(), Error> {
        self.read(offset, data)
    }
}
impl embedded_storage_async::nor_flash::NorFlash for XipFlash {
    const WRITE_SIZE: usize = 1;
    const ERASE_SIZE: usize = SECTOR as usize;
    async fn write(&mut self, offset: u32, data: &[u8]) -> Result<(), Error> {
        self.window.map(offset, data.len())?;
        let mut done = 0;
        while done < data.len() {
            let len = super::logic::page_chunk(
                self.physical_start + offset + done as u32,
                data.len() - done,
            );
            self.write(offset + done as u32, &data[done..done + len])?;
            done += len;
            super::yield_once().await;
        }
        Ok(())
    }
    async fn erase(&mut self, from: u32, to: u32) -> Result<(), Error> {
        self.window.erase_range(from, to)?;
        for offset in (from..to).step_by(SECTOR as usize) {
            self.erase(offset, offset + SECTOR)?;
            super::yield_once().await;
        }
        Ok(())
    }
}
