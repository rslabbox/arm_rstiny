//! Boot archive contract tests over the shared newc decoder.
use rstiny_newc::*;

fn record(bytes: &mut Vec<u8>, name: &[u8], data: &[u8]) -> usize {
    let start = bytes.len();
    bytes.extend_from_slice(b"070701");
    for index in 0..13 {
        let value = match index {
            6 => data.len(),
            11 => name.len() + 1,
            _ => 0,
        };
        bytes.extend_from_slice(format!("{value:08x}").as_bytes());
    }
    bytes.extend_from_slice(name);
    bytes.push(0);
    bytes.resize(bytes.len().next_multiple_of(4), 0);
    bytes.extend_from_slice(data);
    bytes.resize(bytes.len().next_multiple_of(4), 0);
    start
}
fn fixture() -> (Vec<u8>, usize) {
    let mut bytes = Vec::new();
    record(&mut bytes, b"kernel.elf", b"kernel");
    record(&mut bytes, b"kernel.dtb", b"dtb");
    record(&mut bytes, b"userboot", b"root");
    let trailer = record(&mut bytes, b"TRAILER!!!", b"");
    (bytes, trailer)
}
fn rejects(bytes: &[u8], kind: ArchiveErrorKind) -> ArchiveError {
    let error = BootArchive::parse(bytes).unwrap_err();
    assert_eq!(error.kind, kind);
    error
}

#[test]
fn borrows_named_images_and_accepts_zero_block_padding() {
    let (mut bytes, _) = fixture();
    bytes.resize(bytes.len().next_multiple_of(512), 0);
    let archive = BootArchive::parse(&bytes).unwrap();
    assert_eq!(archive.kernel(), b"kernel");
    assert_eq!(archive.device_tree(), b"dtb");
    assert_eq!(archive.rootserver(), b"root");
    assert_eq!(archive.kernel().as_ptr(), bytes[124..].as_ptr());
    let mut unaligned = vec![0];
    unaligned.extend_from_slice(&bytes);
    assert_eq!(
        BootArchive::parse(&unaligned[1..]).unwrap().rootserver(),
        b"root"
    );
}

#[test]
fn all_truncated_prefixes_fail() {
    let (bytes, trailer) = fixture();
    for end in 0..bytes.len() {
        assert!(BootArchive::parse(&bytes[..end]).is_err(), "length {end}");
    }
    assert_eq!(
        rejects(&bytes[..trailer], ArchiveErrorKind::MissingTrailer).offset,
        trailer
    );
}

#[test]
fn validates_header_fields_names_and_extents() {
    let (original, _) = fixture();
    for (offset, replacement, kind) in [
        (0, &b"070702"[..], ArchiveErrorKind::InvalidMagic),
        (6, &b"z"[..], ArchiveErrorKind::InvalidHex),
        (54, &b"ffffffff"[..], ArchiveErrorKind::Truncated),
        (94, &b"ffffffff"[..], ArchiveErrorKind::Truncated),
        (94, &b"00000000"[..], ArchiveErrorKind::InvalidName),
        (94, &b"00000001"[..], ArchiveErrorKind::InvalidName),
        (113, &b"\0"[..], ArchiveErrorKind::InvalidName),
        (120, &b"x"[..], ArchiveErrorKind::InvalidName),
    ] {
        let mut bytes = original.clone();
        bytes[offset..offset + replacement.len()].copy_from_slice(replacement);
        rejects(&bytes, kind);
    }
    let mut bytes = original;
    bytes[6] = b'G';
    let error = rejects(&bytes, ArchiveErrorKind::InvalidHex);
    assert_eq!(error.offset, 6);
    assert!(error.to_string().contains("0x6"));
}

#[test]
fn rejects_wrong_order_duplicates_and_early_trailer() {
    for name in [&b"kernel.dtb"[..], b"TRAILER!!!"] {
        let mut bytes = Vec::new();
        record(&mut bytes, name, b"");
        rejects(&bytes, ArchiveErrorKind::UnexpectedFile);
    }
    let mut bytes = Vec::new();
    record(&mut bytes, b"kernel.elf", b"");
    record(&mut bytes, b"kernel.elf", b"");
    rejects(&bytes, ArchiveErrorKind::UnexpectedFile);
}

#[test]
fn carries_modules_and_enforces_the_trailing_record() {
    let (original, trailer) = fixture();
    let mut bytes = original[..trailer].to_vec();
    record(&mut bytes, b"init.elf", b"init");
    record(&mut bytes, b"init.cfg", b"cfg");
    record(&mut bytes, b"TRAILER!!!", b"");
    let archive = BootArchive::parse(&bytes).unwrap();
    let modules = archive.modules();
    assert_eq!(modules.len(), 2);
    assert_eq!(modules[0], (&b"init.elf"[..], &b"init"[..]));
    assert_eq!(modules[1], (&b"init.cfg"[..], &b"cfg"[..]));
    // The trailer still terminates the archive; truncation before it fails.
    let mut bytes = bytes[..trailer].to_vec();
    rejects(&bytes, ArchiveErrorKind::MissingTrailer);
    let mut bytes = original[..trailer].to_vec();
    record(&mut bytes, b"TRAILER!!!", b"x");
    rejects(&bytes, ArchiveErrorKind::InvalidTrailer);
    let mut bytes = original;
    bytes.extend_from_slice(&[0, 0, 1]);
    assert_eq!(
        rejects(&bytes, ArchiveErrorKind::TrailingData).offset,
        bytes.len() - 1
    );
}
