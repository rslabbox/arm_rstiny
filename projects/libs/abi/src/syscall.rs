//! seL4 non-MCS syscall numbers. AArch64 passes the sign-extended word in x7.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i64)]
pub enum Syscall {
    Call = -1,
    ReplyRecv = -2,
    Send = -3,
    NBSend = -4,
    Recv = -5,
    Reply = -6,
    Yield = -7,
    NBRecv = -8,
    DebugPutchar = -9,
}

impl TryFrom<u64> for Syscall {
    type Error = u64;
    fn try_from(word: u64) -> Result<Self, Self::Error> {
        match word as i64 {
            -1 => Ok(Self::Call),
            -2 => Ok(Self::ReplyRecv),
            -3 => Ok(Self::Send),
            -4 => Ok(Self::NBSend),
            -5 => Ok(Self::Recv),
            -6 => Ok(Self::Reply),
            -7 => Ok(Self::Yield),
            -8 => Ok(Self::NBRecv),
            -9 => Ok(Self::DebugPutchar),
            _ => Err(word),
        }
    }
}
