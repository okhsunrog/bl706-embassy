//! Shared NOR operation engine for blocking/async frontends and host tests.
use core::ops::Range;
pub const PAGE: usize = 256;
pub const SECTOR: u32 = 4096;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    OutOfBounds,
    NotAligned,
    UnsupportedChip,
    WrongDevice,
    Busy,
    ControllerTimeout,
    Timeout,
    Poisoned,
    WriteProtected,
    NeedsErase,
    Verify,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Chip {
    pub jedec_id: [u8; 3],
    pub capacity: u32,
}
impl Chip {
    pub fn identify(id: [u8; 3]) -> Result<Self, Error> {
        if id == [0; 3] || id == [255; 3] || !(0x13..=0x18).contains(&id[2]) {
            return Err(Error::UnsupportedChip);
        }
        Ok(Self {
            jedec_id: id,
            capacity: 1u32 << id[2],
        })
    }
    pub(crate) fn supports_writes(self) -> bool {
        // Only these standard 3-byte SPI NOR families have been reviewed here.
        self.jedec_id == [0xc2, 0x20, 0x14]
            || matches!(
                (self.jedec_id[0], self.jedec_id[1]),
                (0xef, 0x40) | (0xc8, 0x40)
            )
    }
}
pub(crate) struct Window {
    start: u32,
    length: u32,
}
impl Window {
    pub(crate) fn new(chip: Chip, expected: [u8; 3], range: Range<u32>) -> Result<Self, Error> {
        if chip.jedec_id != expected {
            return Err(Error::WrongDevice);
        }
        if !chip.supports_writes() {
            return Err(Error::UnsupportedChip);
        }
        if range.start >= range.end || range.end > chip.capacity {
            return Err(Error::OutOfBounds);
        }
        if !range.start.is_multiple_of(SECTOR) || !range.end.is_multiple_of(SECTOR) {
            return Err(Error::NotAligned);
        }
        Ok(Self {
            start: range.start,
            length: range.end - range.start,
        })
    }
    pub(crate) fn capacity(&self) -> usize {
        self.length as usize
    }
    pub(crate) fn map(&self, offset: u32, length: usize) -> Result<u32, Error> {
        let length = u32::try_from(length).map_err(|_| Error::OutOfBounds)?;
        if offset
            .checked_add(length)
            .is_none_or(|end| end > self.length)
        {
            return Err(Error::OutOfBounds);
        }
        Ok(self.start + offset)
    }
    pub(crate) fn erase_range(&self, from: u32, to: u32) -> Result<(), Error> {
        if from > to {
            return Err(Error::OutOfBounds);
        }
        self.map(from, (to - from) as usize)?;
        if !from.is_multiple_of(SECTOR) || !to.is_multiple_of(SECTOR) {
            return Err(Error::NotAligned);
        }
        Ok(())
    }
}
pub(crate) fn page_chunk(address: u32, remaining: usize) -> usize {
    (PAGE - address as usize % PAGE).min(remaining)
}
/// Returns command word and SAHB control without trigger (SPI 1-1-1, no dummy).
pub(crate) fn command_words(
    opcode: u8,
    address: Option<u32>,
    length: usize,
    write: bool,
) -> Result<(u32, u32), Error> {
    if length > PAGE || address.is_some_and(|n| n >= 1 << 24) {
        return Err(Error::OutOfBounds);
    }
    let mut control = 1 << 27;
    if address.is_some() {
        control |= (1 << 26) | (2 << 17);
    }
    if length != 0 {
        control |= (1 << 24) | (((length - 1) as u32) << 2);
    }
    if write {
        control |= 1 << 23;
    }
    Ok(((u32::from(opcode) << 24) | address.unwrap_or(0), control))
}
pub(crate) trait Transport {
    fn command(
        &mut self,
        opcode: u8,
        address: Option<u32>,
        write: &[u8],
        read: &mut [u8],
    ) -> Result<(), Error>;
    fn now(&self) -> u64;
}
pub(crate) struct Engine<T: Transport> {
    pub(crate) bus: T,
    window: Window,
    pending: Option<u64>,
    poisoned: bool,
}
impl<T: Transport> Engine<T> {
    pub(crate) fn new(bus: T, window: Window) -> Self {
        Self {
            bus,
            window,
            pending: None,
            poisoned: false,
        }
    }
    pub(crate) fn capacity(&self) -> usize {
        self.window.capacity()
    }
    pub(crate) fn can_release(&self) -> bool {
        self.pending.is_none() && !self.poisoned
    }
    pub(crate) fn validate(&self, offset: u32, len: usize) -> Result<(), Error> {
        self.window.map(offset, len).map(|_| ())
    }
    pub(crate) fn validate_erase(&self, from: u32, to: u32) -> Result<(), Error> {
        self.window.erase_range(from, to)
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
        let result = self.bus.command(opcode, address, write, read);
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
    pub(crate) fn poll_ready(&mut self) -> Result<bool, Error> {
        let mut status = [0];
        self.command(5, None, &[], &mut status)?;
        if status[0] & 1 == 0 {
            self.pending = None;
            if status[0] & 2 != 0 {
                self.command(4, None, &[], &mut [])?;
            }
            return Ok(true);
        }
        match self.pending {
            None => Err(Error::Busy),
            Some(deadline) if self.bus.now() >= deadline => {
                self.poisoned = true;
                Err(Error::Timeout)
            }
            Some(_) => Ok(false),
        }
    }
    pub(crate) fn read_piece(&mut self, offset: u32, data: &mut [u8]) -> Result<(), Error> {
        let address = self.window.map(offset, data.len())?;
        if data.len() > PAGE {
            return Err(Error::OutOfBounds);
        }
        if self.pending.is_some() {
            return Err(Error::Busy);
        }
        if data.is_empty() {
            return Ok(());
        }
        self.command(3, Some(address), &[], data)
    }
    fn write_enable(&mut self) -> Result<(), Error> {
        if !self.poll_ready()? {
            return Err(Error::Busy);
        }
        self.command(6, None, &[], &mut [])?;
        let mut status = [0];
        self.command(5, None, &[], &mut status)?;
        if status[0] & 3 != 2 {
            return Err(Error::WriteProtected);
        }
        Ok(())
    }
    pub(crate) fn program_page(&mut self, offset: u32, data: &[u8]) -> Result<(), Error> {
        let address = self.window.map(offset, data.len())?;
        if data.is_empty() || data.len() > page_chunk(address, data.len()) {
            return Err(Error::NotAligned);
        }
        if !self.poll_ready()? {
            return Err(Error::Busy);
        }
        let mut previous = [0; PAGE];
        self.read_piece(offset, &mut previous[..data.len()])?;
        if previous
            .iter()
            .zip(data)
            .any(|(&old, &new)| old & new != new)
        {
            return Err(Error::NeedsErase);
        }
        self.write_enable()?;
        // Persistent state is stored BEFORE issue. Cancelled futures cannot forget that flash is busy.
        self.pending = Some(self.bus.now().saturating_add(10_000));
        self.command(2, Some(address), data, &mut [])
    }
    pub(crate) fn erase_sector(&mut self, offset: u32) -> Result<(), Error> {
        self.window.erase_range(
            offset,
            offset.checked_add(SECTOR).ok_or(Error::OutOfBounds)?,
        )?;
        let address = self.window.map(offset, SECTOR as usize)?;
        self.write_enable()?;
        self.pending = Some(self.bus.now().saturating_add(2_000_000));
        self.command(0x20, Some(address), &[], &mut [])
    }
    pub(crate) fn verify(&mut self, offset: u32, expected: &[u8]) -> Result<(), Error> {
        let mut actual = [0; PAGE];
        self.read_piece(offset, &mut actual[..expected.len()])?;
        if actual[..expected.len()] != *expected {
            return Err(Error::Verify);
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    const ID: [u8; 3] = [0xef, 0x40, 0x13];
    fn chip() -> Chip {
        Chip::identify(ID).unwrap()
    }
    struct Mock {
        data: std::vec::Vec<u8>,
        status: u8,
        busy_polls: u8,
        time: u64,
        commands: std::vec::Vec<(u8, Option<u32>, usize)>,
        protect: bool,
        fail: bool,
    }
    impl Mock {
        fn new() -> Self {
            Self {
                data: std::vec![255;chip().capacity as usize],
                status: 0,
                busy_polls: 0,
                time: 0,
                commands: std::vec::Vec::new(),
                protect: false,
                fail: false,
            }
        }
    }
    impl Transport for Mock {
        fn now(&self) -> u64 {
            self.time
        }
        fn command(
            &mut self,
            op: u8,
            address: Option<u32>,
            write: &[u8],
            read: &mut [u8],
        ) -> Result<(), Error> {
            self.commands.push((op, address, write.len() + read.len()));
            if self.fail {
                return Err(Error::ControllerTimeout);
            }
            match op {
                5 => {
                    read[0] = self.status;
                    if self.busy_polls > 0 {
                        self.busy_polls -= 1;
                        if self.busy_polls == 0 {
                            self.status = 0;
                        }
                    }
                }
                6 => {
                    if !self.protect {
                        self.status |= 2;
                    }
                }
                4 => self.status &= !2,
                3 => {
                    let a = address.unwrap() as usize;
                    read.copy_from_slice(&self.data[a..a + read.len()]);
                }
                2 => {
                    assert_eq!(self.status & 3, 2);
                    let a = address.unwrap() as usize;
                    for (i, &b) in write.iter().enumerate() {
                        self.data[a + i] &= b;
                    }
                    self.status = 1;
                    self.busy_polls = 2;
                }
                0x20 => {
                    assert_eq!(self.status & 3, 2);
                    let a = address.unwrap() as usize;
                    self.data[a..a + SECTOR as usize].fill(255);
                    self.status = 1;
                    self.busy_polls = 2;
                }
                _ => panic!("unexpected command"),
            }
            Ok(())
        }
    }
    fn engine() -> Engine<Mock> {
        Engine::new(
            Mock::new(),
            Window::new(chip(), ID, SECTOR..3 * SECTOR).unwrap(),
        )
    }
    fn ready(e: &mut Engine<Mock>) {
        while !e.poll_ready().unwrap() {}
    }
    #[test]
    fn windows_bounds_alignment_and_overflow() {
        let w = Window::new(chip(), ID, SECTOR..3 * SECTOR).unwrap();
        assert_eq!(w.capacity(), 8192);
        assert_eq!(w.map(8192, 0), Ok(3 * SECTOR));
        for (offset, len) in [(8192, 1), (u32::MAX, 1), (1, usize::MAX)] {
            assert_eq!(w.map(offset, len), Err(Error::OutOfBounds));
        }
        assert_eq!(w.erase_range(0, 4096), Ok(()));
        assert_eq!(w.erase_range(4096, 0), Err(Error::OutOfBounds));
        assert_eq!(w.erase_range(1, 4096), Err(Error::NotAligned));
        assert!(Window::new(chip(), [0; 3], 0..4096).is_err());
        assert!(Window::new(chip(), ID, 1..4096).is_err());
    }
    #[test]
    fn command_fields_do_not_overlap_and_preserve_address_bytes() {
        let (word, control) = command_words(2, Some(0x123456), 256, true).unwrap();
        assert_eq!(word, 0x02123456);
        assert_eq!((control >> 2) & 1023, 255);
        assert_eq!((control >> 17) & 7, 2);
        assert_eq!(control & 3, 0);
        assert_ne!(control & (1 << 23), 0);
        assert_ne!(control & (1 << 24), 0);
        let (_, control) = command_words(6, None, 0, false).unwrap();
        assert_eq!(control, 1 << 27);
        assert!(command_words(3, Some(1 << 24), 1, false).is_err());
        assert!(command_words(3, None, 257, false).is_err());
    }
    #[test]
    fn chunks_never_cross_a_page() {
        for address in 0..1024 {
            for len in 1..1024 {
                let n = page_chunk(address, len);
                assert!(n > 0 && n <= 256 && n <= len);
                assert_eq!(address / 256, (address + n as u32 - 1) / 256);
            }
        }
    }
    #[test]
    fn program_erase_verify_and_region_mapping() {
        let mut e = engine();
        assert_eq!(e.capacity(), 8192);
        assert_eq!(e.validate(8191, 1), Ok(()));
        assert_eq!(e.validate_erase(0, 4096), Ok(()));
        assert!(e.can_release());
        e.program_page(255, &[0x12]).unwrap();
        assert!(!e.can_release());
        assert!(!e.poll_ready().unwrap());
        ready(&mut e);
        assert!(e.can_release());
        e.verify(255, &[0x12]).unwrap();
        assert_eq!(e.bus.data[SECTOR as usize + 255], 0x12);
        assert!(e.bus.data[..SECTOR as usize].iter().all(|&b| b == 255));
        e.erase_sector(0).unwrap();
        ready(&mut e);
        e.verify(255, &[255]).unwrap();
    }
    #[test]
    fn cancellation_keeps_pending_and_new_operation_waits_for_busy() {
        let mut e = engine();
        e.program_page(0, &[0]).unwrap(); // future can be dropped here
        assert!(e.pending.is_some());
        assert_eq!(e.read_piece(0, &mut [0]), Err(Error::Busy));
        assert_eq!(e.program_page(1, &[0]), Err(Error::Busy));
        ready(&mut e);
        e.program_page(1, &[0]).unwrap();
        ready(&mut e);
        e.verify(0, &[0, 0]).unwrap();
    }
    #[test]
    fn failed_preconditions_never_issue_mutating_commands() {
        let mut e = engine();
        assert!(e.program_page(8192, &[0]).is_err());
        assert!(e.program_page(255, &[0, 0]).is_err());
        assert!(e.erase_sector(1).is_err());
        assert!(e.bus.commands.is_empty());
        e.bus.data[SECTOR as usize] = 0;
        assert_eq!(e.program_page(0, &[255]), Err(Error::NeedsErase));
        assert!(
            e.bus
                .commands
                .iter()
                .all(|(op, _, _)| !matches!(op, 2 | 6 | 0x20))
        );
        e.bus.protect = true;
        assert_eq!(e.program_page(1, &[0]), Err(Error::WriteProtected));
        assert!(
            e.bus
                .commands
                .iter()
                .all(|(op, _, _)| !matches!(op, 2 | 0x20))
        );
    }
    #[test]
    fn timeout_and_controller_failure_poison_future_operations() {
        let mut e = engine();
        e.erase_sector(0).unwrap();
        e.bus.time = 2_000_001;
        assert_eq!(e.poll_ready(), Err(Error::Timeout));
        let count = e.bus.commands.len();
        assert_eq!(e.poll_ready(), Err(Error::Poisoned));
        assert_eq!(e.bus.commands.len(), count);
        let mut e = engine();
        e.bus.fail = true;
        assert_eq!(e.poll_ready(), Err(Error::ControllerTimeout));
        assert_eq!(e.poll_ready(), Err(Error::Poisoned));
    }
    #[test]
    fn supported_profiles_and_unknown_busy_are_rejected() {
        assert!(chip().supports_writes());
        assert!(
            Chip::identify([0xc2, 0x20, 0x14])
                .unwrap()
                .supports_writes()
        );
        assert!(
            !Chip::identify([0xc2, 0x20, 0x15])
                .unwrap()
                .supports_writes()
        );
        assert!(
            Chip::identify([0xc8, 0x40, 0x18])
                .unwrap()
                .supports_writes()
        );
        let unknown = Chip::identify([0x85, 0x60, 0x13]).unwrap();
        assert!(!unknown.supports_writes());
        assert!(Window::new(unknown, unknown.jedec_id, 0..4096).is_err());
        assert!(Chip::identify([255; 3]).is_err());
        let mut e = engine();
        e.bus.status = 1;
        assert_eq!(e.poll_ready(), Err(Error::Busy));
    }
}
