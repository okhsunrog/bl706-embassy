//! RAM-only, SPI 1-1-1 NOR flash driver. No XIP boot/image/storage-format implementation.
pub use crate::flash_logic::{Chip, Error};
use crate::{
    clock::Clocks,
    flash_logic::{self as logic, Engine, PAGE, SECTOR, Transport, Window},
    pac,
};
use core::{future::poll_fn, ops::Range, task::Poll};
use embassy_time::{Duration, Timer};
pub struct FlashToken {
    _private: (),
}
impl FlashToken {
    pub(crate) const fn new() -> Self {
        Self { _private: () }
    }
}

struct Controller {
    _token: FlashToken,
    saved_clock: u32,
    saved_control0: u32,
    saved_control1: u32,
    poisoned: bool,
}
impl Controller {
    fn idle(&mut self) -> Result<(), Error> {
        let deadline = crate::time::now_ticks().saturating_add(1_000);
        for _ in 0..100_000 {
            if pac::SF_CTRL.sf_if_sahb_0().read().0 & 1 == 0 {
                return Ok(());
            }
            if crate::time::now_ticks() >= deadline {
                break;
            }
        }
        self.poisoned = true;
        Err(Error::ControllerTimeout)
    }
    fn new(token: FlashToken, clocks: Clocks) -> Result<Self, Error> {
        critical_section::with(|_| pac::GLB.cgen_cfg1().modify(|r| r.0 |= 1 << 11));
        crate::io_fence();
        let mut bus = Self {
            _token: token,
            saved_clock: pac::GLB.clk_cfg2().read().0,
            saved_control0: pac::SF_CTRL.sf_ctrl_0().read().0,
            saved_control1: pac::SF_CTRL.sf_ctrl_1().read().0,
            poisoned: false,
        };
        bus.idle()?;
        critical_section::with(|_| {
            pac::GLB.clk_cfg2().modify(|r| r.set_sf_clk_en(false));
            pac::GLB.clk_cfg2().modify(|r| {
                r.set_sf_clk_sel(0);
                r.set_sf_clk_sel2(1);
                r.set_sf_clk_div((clocks.xclk_hz() / 8_000_000 - 1) as u8);
                r.set_sf_clk_en(true);
            });
        });
        pac::SF_CTRL.sf_ctrl_1().modify(|r| {
            r.set_sf_if_fn_sel(false);
            r.set_sf_ahb2sif_en(false);
        });
        pac::SF_CTRL
            .sf_ctrl_0()
            .modify(|r| r.set_sf_clk_sahb_sram_sel(false));
        crate::io_fence();
        Ok(bus)
    }
}
impl Transport for Controller {
    fn now(&self) -> u64 {
        crate::time::now_ticks()
    }
    fn command(
        &mut self,
        opcode: u8,
        address: Option<u32>,
        write: &[u8],
        read: &mut [u8],
    ) -> Result<(), Error> {
        if self.poisoned {
            return Err(Error::Poisoned);
        }
        if !write.is_empty() && !read.is_empty() {
            return Err(Error::OutOfBounds);
        }
        let (word, control) = logic::command_words(
            opcode,
            address,
            write.len().max(read.len()),
            !write.is_empty(),
        )?;
        self.idle()?;
        pac::SF_CTRL.sf_if_sahb_0().write(|r| r.0 = control); // lower TRIG before touching command/buffer
        // SF_CTRL SRAM is a 256-byte peripheral window. No DMA points into caller memory.
        pac::SF_CTRL
            .sf_ctrl_0()
            .modify(|r| r.set_sf_clk_sahb_sram_sel(false));
        crate::io_fence();
        for i in 0..write.len().div_ceil(4) {
            let mut value = 0u32;
            for j in 0..4 {
                value |= u32::from(write.get(i * 4 + j).copied().unwrap_or(0)) << (j * 8);
            }
            unsafe {
                (0x4000_b700 as *mut u32).add(i).write_volatile(value);
            }
        }
        pac::SF_CTRL.sf_if_sahb_0().write(|r| r.0 = control);
        pac::SF_CTRL.sf_if_sahb_1().write(|r| r.0 = word);
        pac::SF_CTRL.sf_if_sahb_2().write(|r| r.0 = 0);
        pac::SF_CTRL
            .sf_ctrl_0()
            .modify(|r| r.set_sf_clk_sahb_sram_sel(true));
        crate::io_fence();
        pac::SF_CTRL.sf_if_sahb_0().write(|r| r.0 = control | 2);
        crate::io_fence();
        self.idle()?;
        pac::SF_CTRL
            .sf_ctrl_0()
            .modify(|r| r.set_sf_clk_sahb_sram_sel(false));
        crate::io_fence();
        for i in 0..read.len().div_ceil(4) {
            let value = unsafe { (0x4000_b700 as *const u32).add(i).read_volatile() };
            for j in 0..4 {
                if let Some(byte) = read.get_mut(i * 4 + j) {
                    *byte = (value >> (j * 8)) as u8;
                }
            }
        }
        Ok(())
    }
}
impl Drop for Controller {
    fn drop(&mut self) {
        if self.poisoned || pac::SF_CTRL.sf_if_sahb_0().read().0 & 1 != 0 {
            return;
        }
        // Restore only fields owned here. This is NOT an XIP resume/cache coherency protocol.
        pac::SF_CTRL
            .sf_ctrl_0()
            .write(|r| r.0 = self.saved_control0);
        pac::SF_CTRL
            .sf_ctrl_1()
            .write(|r| r.0 = self.saved_control1);
        let saved = pac::glb::regs::clk_cfg2(self.saved_clock);
        critical_section::with(|_| {
            pac::GLB.clk_cfg2().modify(|r| {
                r.set_sf_clk_sel(saved.sf_clk_sel());
                r.set_sf_clk_sel2(saved.sf_clk_sel2());
                r.set_sf_clk_div(saved.sf_clk_div());
                r.set_sf_clk_en(saved.sf_clk_en());
            })
        });
        crate::io_fence();
    }
}

/// Read-only identification handle. Region conversion requires explicit device ID and bounds.
pub struct Probe {
    bus: Controller,
    chip: Chip,
}
impl Probe {
    /// Acquire a ROM/board-initialized SPI flash interface at nominal XCLK/4=8MHz.
    ///
    /// # Safety
    /// All executing code, constants, interrupt handlers, stack and buffers must reside in RAM/ROM.
    /// No hart/DMA/debug access may use SF_CTRL/XIP/flash mappings during this ownership.
    /// Flash pads/timing must already be initialized for this board; the device must be awake
    /// in SPI 1-1-1 mode, outside QPI/continuous-read mode. This driver does not establish those
    /// board-specific conditions or resume XIP. Remain RAM-only until a reset/reinitialization.
    /// Keep flash pads and their configuration exclusively reserved for the handle's lifetime.
    pub unsafe fn new(token: FlashToken, clocks: Clocks) -> Result<Self, Error> {
        let mut bus = Controller::new(token, clocks)?;
        let mut id = [0; 3];
        bus.command(0x9f, None, &[], &mut id)?;
        let chip = Chip::identify(id)?;
        let mut status = [0];
        bus.command(5, None, &[], &mut status)?;
        if status[0] & 1 != 0 {
            return Err(Error::Busy);
        }
        Ok(Self { bus, chip })
    }
    pub fn chip(&self) -> Chip {
        self.chip
    }
    pub fn read(&mut self, offset: u32, data: &mut [u8]) -> Result<(), Error> {
        let length = u32::try_from(data.len()).map_err(|_| Error::OutOfBounds)?;
        if offset
            .checked_add(length)
            .is_none_or(|end| end > self.chip.capacity)
        {
            return Err(Error::OutOfBounds);
        }
        for (i, chunk) in data.chunks_mut(PAGE).enumerate() {
            self.bus
                .command(3, Some(offset + (i * PAGE) as u32), &[], chunk)?;
        }
        Ok(())
    }
    /// Reserve an explicitly chosen erase-aligned region. Offsets in NorFlash are region-relative.
    /// Only reviewed Winbond W25Q (EF40) and GigaDevice GD25Q (C840), 512KiB..16MiB,
    /// are accepted for writes. This does not assert which chip is fitted on a Pinecil.
    pub fn into_region(self, expected_jedec: [u8; 3], region: Range<u32>) -> Result<Flash, Error> {
        let window = Window::new(self.chip, expected_jedec, region)?;
        Ok(Flash {
            engine: Engine::new(self.bus, window),
        })
    }
}

pub struct Flash {
    engine: Engine<Controller>,
}
impl Drop for Flash {
    fn drop(&mut self) {
        // A cancelled operation may still be changing the array. Do not restore an XIP owner.
        if !self.engine.can_release() {
            self.engine.bus.poisoned = true;
        }
    }
}
impl Flash {
    pub fn capacity(&self) -> usize {
        self.engine.capacity()
    }
    fn ready(&mut self) -> Result<(), Error> {
        while !self.engine.poll_ready()? {
            core::hint::spin_loop();
        }
        Ok(())
    }
    async fn ready_async(&mut self) -> Result<(), Error> {
        while !self.engine.poll_ready()? {
            Timer::after(Duration::from_millis(1)).await;
        }
        Ok(())
    }
    pub fn read(&mut self, offset: u32, data: &mut [u8]) -> Result<(), Error> {
        self.engine.validate(offset, data.len())?;
        for (i, chunk) in data.chunks_mut(PAGE).enumerate() {
            self.ready()?;
            self.engine.read_piece(offset + (i * PAGE) as u32, chunk)?;
        }
        Ok(())
    }
    pub fn write(&mut self, offset: u32, data: &[u8]) -> Result<(), Error> {
        self.engine.validate(offset, data.len())?;
        let mut done = 0;
        while done < data.len() {
            self.ready()?;
            let position = offset + done as u32;
            let len = logic::page_chunk(position, data.len() - done);
            let page = &data[done..done + len];
            self.engine.program_page(position, page)?;
            self.ready()?;
            self.engine.verify(position, page)?;
            done += len;
        }
        Ok(())
    }
    pub fn erase(&mut self, from: u32, to: u32) -> Result<(), Error> {
        self.engine.validate_erase(from, to)?;
        for offset in (from..to).step_by(SECTOR as usize) {
            self.ready()?;
            self.engine.erase_sector(offset)?;
            self.ready()?;
            for page in (offset..offset + SECTOR).step_by(PAGE) {
                self.engine.verify(page, &[255; PAGE])?;
            }
        }
        Ok(())
    }
    pub async fn read_async(&mut self, offset: u32, data: &mut [u8]) -> Result<(), Error> {
        self.engine.validate(offset, data.len())?;
        for (i, chunk) in data.chunks_mut(PAGE).enumerate() {
            self.ready_async().await?;
            self.engine.read_piece(offset + (i * PAGE) as u32, chunk)?;
            yield_once().await;
        }
        Ok(())
    }
    pub async fn write_async(&mut self, offset: u32, data: &[u8]) -> Result<(), Error> {
        self.engine.validate(offset, data.len())?;
        let mut done = 0;
        while done < data.len() {
            self.ready_async().await?;
            let position = offset + done as u32;
            let len = logic::page_chunk(position, data.len() - done);
            let page = &data[done..done + len];
            self.engine.program_page(position, page)?;
            self.ready_async().await?;
            self.engine.verify(position, page)?;
            done += len;
            yield_once().await;
        }
        Ok(())
    }
    pub async fn erase_async(&mut self, from: u32, to: u32) -> Result<(), Error> {
        self.engine.validate_erase(from, to)?;
        for offset in (from..to).step_by(SECTOR as usize) {
            self.ready_async().await?;
            self.engine.erase_sector(offset)?;
            self.ready_async().await?;
            for page in (offset..offset + SECTOR).step_by(PAGE) {
                self.engine.verify(page, &[255; PAGE])?;
                yield_once().await;
            }
        }
        Ok(())
    }
}
async fn yield_once() {
    let mut yielded = false;
    poll_fn(|cx| {
        if yielded {
            Poll::Ready(())
        } else {
            yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    })
    .await;
}
impl embedded_storage::nor_flash::NorFlashError for Error {
    fn kind(&self) -> embedded_storage::nor_flash::NorFlashErrorKind {
        use embedded_storage::nor_flash::NorFlashErrorKind as K;
        match self {
            Self::OutOfBounds => K::OutOfBounds,
            Self::NotAligned => K::NotAligned,
            _ => K::Other,
        }
    }
}
impl embedded_storage::nor_flash::ErrorType for Flash {
    type Error = Error;
}
impl embedded_storage::nor_flash::ReadNorFlash for Flash {
    const READ_SIZE: usize = 1;
    fn capacity(&self) -> usize {
        self.capacity()
    }
    fn read(&mut self, offset: u32, data: &mut [u8]) -> Result<(), Error> {
        self.read(offset, data)
    }
}
impl embedded_storage::nor_flash::NorFlash for Flash {
    const WRITE_SIZE: usize = 1;
    const ERASE_SIZE: usize = SECTOR as usize;
    fn write(&mut self, offset: u32, data: &[u8]) -> Result<(), Error> {
        self.write(offset, data)
    }
    fn erase(&mut self, from: u32, to: u32) -> Result<(), Error> {
        self.erase(from, to)
    }
}
impl embedded_storage_async::nor_flash::ReadNorFlash for Flash {
    const READ_SIZE: usize = 1;
    fn capacity(&self) -> usize {
        self.capacity()
    }
    async fn read(&mut self, offset: u32, data: &mut [u8]) -> Result<(), Error> {
        self.read_async(offset, data).await
    }
}
impl embedded_storage_async::nor_flash::NorFlash for Flash {
    const WRITE_SIZE: usize = 1;
    const ERASE_SIZE: usize = SECTOR as usize;
    async fn write(&mut self, offset: u32, data: &[u8]) -> Result<(), Error> {
        self.write_async(offset, data).await
    }
    async fn erase(&mut self, from: u32, to: u32) -> Result<(), Error> {
        self.erase_async(from, to).await
    }
}
