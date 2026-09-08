use rs_fdtree::LinuxFdt;

const DATA: &[u8] = include_bytes!("../dtb/test.dtb");
fn word(data: &[u8], offset: usize) -> usize {
    u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap()) as usize
}
fn put(data: &mut [u8], offset: usize, value: u32) {
    data[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

#[test]
fn rejects_bad_block_ranges_and_truncation() {
    for len in [0, 39, DATA.len() - 1] {
        assert!(LinuxFdt::new(&DATA[..len]).is_err());
    }
    for offset in [4, 8, 12, 16, 32, 36] {
        let mut data = DATA.to_vec();
        put(&mut data, offset, u32::MAX);
        assert!(LinuxFdt::new(&data).is_err(), "offset {offset}");
    }
}

#[test]
fn rejects_bad_property_and_end_marker() {
    let start = word(DATA, 8);
    let size = word(DATA, 36);
    let prop = (start..start + size)
        .step_by(4)
        .find(|p| word(DATA, *p) == 3)
        .unwrap();
    for offset in [prop + 4, prop + 8] {
        let mut data = DATA.to_vec();
        put(&mut data, offset, u32::MAX);
        assert!(LinuxFdt::new(&data).is_err());
    }
    let mut data = DATA.to_vec();
    put(&mut data, start + size - 4, 5);
    assert!(LinuxFdt::new(&data).is_err());
}

#[test]
fn bounded_depth() {
    // A synthetic tree deeper than the parser's fixed parent stack.
    let mut structure = Vec::new();
    for _ in 0..64 {
        structure.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 0]);
    }
    for _ in 0..64 {
        structure.extend_from_slice(&2u32.to_be_bytes());
    }
    structure.extend_from_slice(&9u32.to_be_bytes());
    let mut data = vec![0; 56];
    data.extend_from_slice(&structure);
    let size = data.len() as u32;
    for (offset, value) in [
        (0, 0xd00dfeed),
        (4, size),
        (8, 56),
        (12, size),
        (16, 40),
        (20, 17),
        (24, 16),
        (36, structure.len() as u32),
    ] {
        put(&mut data, offset, value);
    }
    assert!(LinuxFdt::new(&data).is_err());
}

#[test]
fn mutated_inputs_never_panic_when_traversed() {
    let mut seed = 1u32;
    for _ in 0..2000 {
        let mut data = DATA.to_vec();
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        let index = seed as usize % data.len();
        data[index] ^= (seed >> 24) as u8;
        if let Ok(tree) = LinuxFdt::new(&data) {
            for node in tree.all_nodes() {
                let _ = node.properties().count();
            }
            let _ = tree.find_node("/does-not-exist");
            let _ = tree.mem_reservations().count();
        }
    }
}
