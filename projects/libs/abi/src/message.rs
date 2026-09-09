//! seL4 64-bit MessageInfo and IPC buffer wire layout.
pub const MAX_MESSAGE_WORDS: usize = 120;
pub const MAX_EXTRA_CAPS: usize = 3;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct MessageInfo(u64);
impl MessageInfo {
    pub const fn new(label: u64, extra_caps: usize, length: usize) -> Self {
        assert!(label < (1 << 52) && extra_caps <= MAX_EXTRA_CAPS && length <= MAX_MESSAGE_WORDS);
        Self((label << 12) | ((extra_caps as u64) << 7) | length as u64)
    }
    pub const fn from_word(word: u64) -> Self {
        Self(word)
    }
    pub const fn word(self) -> u64 {
        self.0
    }
    pub const fn label(self) -> u64 {
        self.0 >> 12
    }
    pub const fn length(self) -> usize {
        (self.0 & 0x7f) as usize
    }
    pub const fn extra_caps(self) -> usize {
        ((self.0 >> 7) & 3) as usize
    }
    pub const fn caps_unwrapped(self) -> u64 {
        (self.0 >> 9) & 7
    }
}
#[repr(C, align(1024))]
pub struct IpcBuffer {
    pub tag: MessageInfo,
    pub msg: [u64; MAX_MESSAGE_WORDS],
    pub user_data: u64,
    pub caps_or_badges: [u64; MAX_EXTRA_CAPS],
    pub receive_cnode: u64,
    pub receive_index: u64,
    pub receive_depth: u64,
}
const _: () = {
    assert!(core::mem::size_of::<IpcBuffer>() == 1024);
    assert!(core::mem::offset_of!(IpcBuffer, caps_or_badges) == 976);
};
