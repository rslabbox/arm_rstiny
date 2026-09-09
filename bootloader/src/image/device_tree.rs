//! Bounded DTB header view; device discovery remains a build-time responsibility.
#[derive(Debug, Clone, Copy)]
pub enum Error {
    Header,
    Size,
}
pub struct DeviceTree<'a>(&'a [u8]);
impl<'a> DeviceTree<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() < 40 || bytes[..4] != [0xd0, 0x0d, 0xfe, 0xed] {
            return Err(Error::Header);
        }
        let size = u32::from_be_bytes(bytes[4..8].try_into().map_err(|_| Error::Header)?) as usize;
        if !(40..=1024 * 1024).contains(&size) {
            return Err(Error::Size);
        }
        Ok(Self(bytes.get(..size).ok_or(Error::Size)?))
    }
    pub fn bytes(&self) -> &'a [u8] {
        self.0
    }
}
