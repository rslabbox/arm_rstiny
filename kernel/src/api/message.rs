//! Kernel object request validation and reply marshalling.
use crate::{arch::kernel::thread::user::UserContext, task::api};
use kernel_abi::*;
pub(crate) struct Request {
    pub label: u64,
    pub words: [u64; kernel_abi::MAX_MESSAGE_WORDS],
    pub length: usize,
    pub caps: [u64; kernel_abi::MAX_EXTRA_CAPS],
    pub extra_caps: usize,
}
impl Request {
    pub fn require(&self, words: usize, caps: usize) -> Result<(), u64> {
        if self.length < words || self.extra_caps < caps {
            Err(kernel_abi::TRUNCATED_MESSAGE)
        } else {
            Ok(())
        }
    }
}
pub(crate) struct Completion {
    pub value: Option<u64>,
    pub disposition: crate::task::Disposition,
}
impl Completion {
    pub fn done(value: Option<u64>) -> Self {
        Self {
            value,
            disposition: crate::task::Disposition::Resume,
        }
    }
    pub fn park(disposition: crate::task::Disposition) -> Self {
        Self {
            value: None,
            disposition,
        }
    }
}

pub(super) fn request(context: &UserContext) -> Result<Request, u64> {
    let info = MessageInfo::from_word(context.message_info());
    if info.length() > MAX_MESSAGE_WORDS || info.caps_unwrapped() != 0 {
        return Err(TRUNCATED_MESSAGE);
    }
    let mut request = Request {
        label: info.label(),
        words: [0; MAX_MESSAGE_WORDS],
        length: info.length(),
        caps: [0; MAX_EXTRA_CAPS],
        extra_caps: info.extra_caps(),
    };
    for index in 0..request.length.min(4) {
        request.words[index] = context.message_register(index);
    }
    for index in 4..request.length {
        let mut bytes = [0; 8];
        api::ipc_read(8 + index * 8, &mut bytes)?;
        request.words[index] = u64::from_le_bytes(bytes);
    }
    for index in 0..request.extra_caps {
        let mut bytes = [0; 8];
        api::ipc_read(976 + index * 8, &mut bytes)?;
        request.caps[index] = u64::from_le_bytes(bytes);
    }
    Ok(request)
}

pub(super) fn reply(context: &mut UserContext, label: u64, words: &[u64]) {
    // Kernel object replies carry no badge.
    context.set_reply(0, MessageInfo::new(label, 0, words.len()).word(), words);
}
