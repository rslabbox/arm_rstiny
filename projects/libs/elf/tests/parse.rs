use rstiny_elf::{Elf, Error, Header};

fn put(bytes: &mut [u8], offset: usize, value: u64, size: usize) {
    bytes[offset..offset + size].copy_from_slice(&value.to_le_bytes()[..size]);
}
fn fixture() -> Vec<u8> {
    let mut bytes = vec![0; 0x2000];
    bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    for (offset, value, size) in [
        (16, 2, 2),
        (18, 183, 2),
        (20, 1, 4),
        (24, 0x400000, 8),
        (32, 64, 8),
        (52, 64, 2),
        (54, 56, 2),
        (56, 1, 2),
        (64, 1, 4),
        (68, 5, 4),
        (72, 0x1000, 8),
        (80, 0x400000, 8),
        (88, 0x40200000, 8),
        (96, 0x1000, 8),
        (104, 0x2000, 8),
        (112, 0x1000, 8),
    ] {
        put(&mut bytes, offset, value, size);
    }
    bytes
}

#[test]
fn borrowed_unaligned_and_separately_read_headers() {
    let bytes = fixture();
    let mut unaligned = vec![0];
    unaligned.extend_from_slice(&bytes);
    let elf = Elf::parse(&unaligned[1..]).unwrap();
    assert_eq!(
        (elf.start(), elf.end(), elf.entry()),
        (0x400000, 0x402000, 0x400000)
    );
    let s = elf.segments().next().unwrap();
    assert_eq!((s.filesz, s.memsz, s.pa), (0x1000, 0x2000, 0x40200000));
    let header = Header::parse(&bytes[..64]).unwrap();
    let table = bytes[header.program_headers_range()].to_vec();
    let split = Elf::from_headers(header, &table, bytes.len()).unwrap();
    assert_eq!(split.entry(), elf.entry());
    assert_eq!(split.program_headers(), elf.program_headers());
    assert_eq!(split.program_header_count(), 1);
    assert_eq!(
        Elf::from_headers(header, &table[..55], bytes.len()).unwrap_err(),
        Error::HeaderTableSize
    );
    assert_eq!(
        Elf::from_headers(header, &table, 100).unwrap_err(),
        Error::Truncated
    );
}

#[test]
fn every_truncated_prefix_is_rejected_without_panicking() {
    let bytes = fixture();
    for end in 0..bytes.len() {
        assert!(Elf::parse(&bytes[..end]).is_err(), "length {end}");
    }
}

#[test]
fn malformed_metadata_is_rejected() {
    let cases = [
        (18, 62, 2, Error::UnsupportedHeader),
        (54, 0, 2, Error::UnsupportedHeader),
        (56, 33, 2, Error::HeaderCount),
        (32, u64::MAX, 8, Error::Overflow),
        (64, 3, 4, Error::UnsupportedFeature),
        (96, 0x3000, 8, Error::InvalidSegment),
        (104, 0, 8, Error::InvalidSegment),
        (72, u64::MAX, 8, Error::InvalidSegment),
        (80, u64::MAX - 0x1000, 8, Error::Overflow),
        (88, u64::MAX, 8, Error::InvalidSegment),
        (112, 0x1800, 8, Error::InvalidSegment),
        (80, 0x400001, 8, Error::InvalidSegment),
        (24, 0x401000, 8, Error::InvalidEntry), // BSS is not an entry point.
        (24, 0x400001, 8, Error::InvalidEntry),
        (68, 4, 4, Error::InvalidEntry),
    ];
    for (offset, value, size, expected) in cases {
        let mut bytes = fixture();
        put(&mut bytes, offset, value, size);
        assert_eq!(Elf::parse(&bytes).unwrap_err(), expected, "offset {offset}");
    }
}

#[test]
fn overlapping_pages_rejected_but_address_policy_belongs_to_loader() {
    let mut bytes = fixture();
    put(&mut bytes, 56, 2, 2);
    bytes.copy_within(64..120, 120);
    put(&mut bytes, 120 + 16, 0x401000, 8);
    assert_eq!(Elf::parse(&bytes).unwrap_err(), Error::OverlappingSegments);
    put(&mut bytes, 120 + 16, 0x402000, 8);
    assert_eq!(Elf::parse(&bytes).unwrap().segments().count(), 2);

    let mut bytes = fixture();
    let high = 0xffff000040200000;
    put(&mut bytes, 24, high, 8);
    put(&mut bytes, 80, high, 8);
    put(&mut bytes, 68, 7, 4); // W^X is checked by each loader, not the decoder.
    assert_eq!(Elf::parse(&bytes).unwrap().entry(), high as usize);
}
