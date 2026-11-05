use bt_hci::transport::PacketKind;

/// No HCI event can exceed 257 bytes. Four extra bytes cover the ACL header.
pub const CAPACITY: usize = 260;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidPacket,
    Unsupported,
    QueueFull,
    Vendor(i8),
}
impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl core::error::Error for Error {}
impl embedded_io::Error for Error {
    fn kind(&self) -> embedded_io::ErrorKind {
        embedded_io::ErrorKind::Other
    }
}

/// Owned callback data: vendor message storage may disappear on return.
pub struct Packet {
    pub kind: PacketKind,
    bytes: [u8; CAPACITY],
    len: usize,
}
impl Packet {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
    pub(crate) fn empty(kind: PacketKind) -> Self {
        Self {
            kind,
            bytes: [0; CAPACITY],
            len: 0,
        }
    }
    #[cfg(any(test, all(feature = "controller", target_arch = "riscv32")))]
    pub(crate) fn acl_storage(&mut self) -> &mut [u8; CAPACITY] {
        &mut self.bytes
    }
    #[cfg(any(test, all(feature = "controller", target_arch = "riscv32")))]
    pub(crate) fn set_acl_len(&mut self, len: usize) -> Result<(), Error> {
        if !(4..=CAPACITY).contains(&len)
            || usize::from(u16::from_le_bytes([self.bytes[2], self.bytes[3]])) + 4 != len
        {
            return Err(Error::InvalidPacket);
        }
        self.len = len;
        Ok(())
    }
    /// Reconstruct the event header stripped by on-chip HCI.
    pub fn event(native_kind: u8, source: u16, params: &[u8]) -> Result<Self, Error> {
        let mut p = Self::empty(PacketKind::Event);
        let opcode = source.to_le_bytes();
        let prefix: &[u8] = match native_kind {
            2 if params.len() <= 252 => &[0x0e, (params.len() + 3) as u8, 1, opcode[0], opcode[1]],
            3 if params.len() == 1 => &[0x0f, 4, params[0], 1, opcode[0], opcode[1]],
            4 if params.len() <= 255 => &[0x3e, params.len() as u8],
            5 if params.len() <= 255 && source <= 255 => &[source as u8, params.len() as u8],
            _ => return Err(Error::InvalidPacket),
        };
        p.bytes[..prefix.len()].copy_from_slice(prefix);
        p.len = prefix.len();
        if native_kind != 3 {
            p.bytes[p.len..p.len + params.len()].copy_from_slice(params);
            p.len += params.len();
        }
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn acl_header_length_is_checked() {
        let mut p = Packet::empty(PacketKind::AclData);
        p.acl_storage()[..6].copy_from_slice(&[0, 0x20, 2, 0, 1, 2]);
        assert!(p.set_acl_len(5).is_err());
        p.set_acl_len(6).unwrap();
        assert_eq!(p.bytes(), &[0, 0x20, 2, 0, 1, 2]);
    }
    #[test]
    fn complete_retains_opcode_and_status() {
        assert_eq!(
            Packet::event(2, 0x0c03, &[0]).unwrap().bytes(),
            &[0x0e, 4, 1, 3, 12, 0]
        );
    }
    #[test]
    fn status_has_no_duplicate_payload() {
        assert_eq!(
            Packet::event(3, 0x200d, &[0x0c]).unwrap().bytes(),
            &[0x0f, 4, 0x0c, 1, 0x0d, 0x20]
        );
    }
    #[test]
    fn rejects_unrepresentable_lengths_and_missing_status() {
        assert!(Packet::event(2, 0, &[0; 253]).is_err());
        assert!(Packet::event(3, 0, &[]).is_err());
        assert!(Packet::event(5, 256, &[]).is_err());
    }
    #[test]
    fn le_subevent_is_already_in_payload() {
        assert_eq!(
            Packet::event(4, 0, &[1, 2, 3]).unwrap().bytes(),
            &[0x3e, 3, 1, 2, 3]
        );
    }
    #[test]
    fn maximum_event_fits() {
        assert_eq!(
            Packet::event(5, 0xff, &[0; 255]).unwrap().bytes().len(),
            257
        );
    }
}

impl<E: embedded_io::Error> From<bt_hci::ReadHciError<E>> for Error {
    fn from(_: bt_hci::ReadHciError<E>) -> Self {
        Self::InvalidPacket
    }
}

/// Native on-chip HCI routes selected link-controller commands by handle.
/// Matches the SDK bl_hci_wrapper.c switch (not all handle-bearing commands).
#[cfg(any(test, all(feature = "controller", target_arch = "riscv32")))]
pub(crate) fn command_destination(opcode: u16, parameters: &[u8]) -> Result<u16, Error> {
    let routed = matches!(
        opcode,
        0x2013 | // LE Connection Update
        0x2015 | // LE Read Channel Map
        0x2016 | // LE Read Remote Features
        0x2019 | // LE Start Encryption
        0x201a | 0x201b | // LE LTK Reply / Negative Reply
        0x2020 | 0x2021 | // LE Connection Parameter Reply / Negative Reply
        0x2022 | // LE Set Data Length
        0x2030 | 0x2032 | // LE Read / Set PHY
        0x041b | 0x041c | // Read Remote / Extended Features
        0x1408 // Read Encryption Key Size
    );
    if routed {
        let handle = parameters.get(..2).ok_or(Error::InvalidPacket)?;
        Ok(u16::from_le_bytes([handle[0], handle[1]]) & 0x0fff)
    } else {
        Ok(0)
    }
}
#[cfg(test)]
mod routing_tests {
    use super::*;
    #[test]
    fn routes_connection_commands_and_rejects_missing_handle() {
        assert_eq!(command_destination(0x2022, &[0x34, 0x02]).unwrap(), 0x234);
        assert_eq!(command_destination(0x1009, &[]).unwrap(), 0);
        assert!(command_destination(0x2022, &[0]).is_err());
    }
}
