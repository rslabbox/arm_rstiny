//! Wire fixtures from seL4's non-MCS AArch64 syscall/XML/bitfield definitions.
use kernel_abi::*;

#[test]
fn syscall_register_words() {
    for (call, wire) in [
        (Syscall::Call, 0xffff_ffff_ffff_ffff),
        (Syscall::ReplyRecv, 0xffff_ffff_ffff_fffe),
        (Syscall::Send, 0xffff_ffff_ffff_fffd),
        (Syscall::NBSend, 0xffff_ffff_ffff_fffc),
        (Syscall::Recv, 0xffff_ffff_ffff_fffb),
        (Syscall::Reply, 0xffff_ffff_ffff_fffa),
        (Syscall::Yield, 0xffff_ffff_ffff_fff9),
        (Syscall::NBRecv, 0xffff_ffff_ffff_fff8),
        (Syscall::DebugPutchar, 0xffff_ffff_ffff_fff7),
    ] {
        assert_eq!(call as i64 as u64, wire);
        assert_eq!(Syscall::try_from(wire), Ok(call));
    }
    assert!(Syscall::try_from(0).is_err());
    assert!(Syscall::try_from(1).is_err());
}

#[test]
fn object_request_and_reply_tags() {
    // Untyped_Retype: six message words and one destination CNode capability.
    assert_eq!(MessageInfo::new(1, 1, 6).word(), 0x1086);
    // TCB_Configure: four words and three extra capabilities.
    assert_eq!(MessageInfo::new(5, 3, 4).word(), 0x5184);
    // MessageInfo decodes unwrapped caps independently from extra caps/length.
    let tag = MessageInfo::from_word(0x1234_fb8);
    assert_eq!(tag.label(), 0x1234);
    assert_eq!(tag.caps_unwrapped(), 7);
    assert_eq!(tag.extra_caps(), 3);
    assert_eq!(tag.length(), 56);
    assert_eq!(MessageInfo::new(NOT_FOUND, 0, 1).word(), 0x6001);
}

#[test]
fn ipc_buffer_binary_layout() {
    use core::mem::{align_of, offset_of, size_of};
    assert_eq!(size_of::<IpcBuffer>(), 1024);
    assert_eq!(align_of::<IpcBuffer>(), 1024);
    assert_eq!(offset_of!(IpcBuffer, msg), 8);
    assert_eq!(offset_of!(IpcBuffer, user_data), 968);
    assert_eq!(offset_of!(IpcBuffer, caps_or_badges), 976);
    assert_eq!(offset_of!(IpcBuffer, receive_cnode), 1000);
    assert_eq!(offset_of!(IpcBuffer, receive_index), 1008);
    assert_eq!(offset_of!(IpcBuffer, receive_depth), 1016);
}
