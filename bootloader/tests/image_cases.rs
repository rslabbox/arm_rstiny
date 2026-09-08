use super::*;
fn elf(virtual_start: usize, root: bool) -> Vec<u8> {
    let mut data = vec![0u8; 8192];
    data[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    let mut put = |offset: usize, value: usize, size: usize| {
        data[offset..offset + size].copy_from_slice(&(value as u64).to_le_bytes()[..size]);
    };
    for (offset, value, size) in [
        (16, 2, 2),
        (18, 183, 2),
        (20, 1, 4),
        (24, virtual_start, 8),
        (32, 64, 8),
        (52, 64, 2),
        (54, 56, 2),
        (56, if root { 2 } else { 1 }, 2),
        (64, 1, 4),
        (68, 5, 4),
        (72, 4096, 8),
        (80, virtual_start, 8),
        (88, 0xdead0000, 8), // p_paddr must not determine allocation.
        (96, 4, 8),
        (104, 4096, 8),
        (112, 4096, 8),
    ] {
        put(offset, value, size);
    }
    if root {
        for (offset, value, size) in [
            (120, 1, 4),
            (124, 6, 4),
            (128, 8192, 8),
            (136, virtual_start + 0x3000, 8),
            (160, 0x4000, 8),
            (168, 4096, 8),
        ] {
            put(offset, value, size);
        }
    }
    data
}
fn dtb() -> Vec<u8> {
    let mut data = vec![0; 40];
    data[..4].copy_from_slice(&0xd00dfeedu32.to_be_bytes());
    data[4..8].copy_from_slice(&40u32.to_be_bytes());
    data
}
fn images<'a>(kernel: &'a [u8], root: &'a [u8], dtb: &'a [u8]) -> BootImages<'a> {
    BootImages {
        kernel: Elf::parse(kernel).unwrap(),
        root: Elf::parse(root).unwrap(),
        dtb: DeviceTree::parse(dtb).unwrap(),
    }
}
#[test]
fn plan_moves_the_entire_image_set_and_ignores_elf_physical_addresses() {
    let kernel = elf(platform::KERNEL_VA_START, false);
    let root = elf(0x10000, true);
    let dtb = dtb();
    let loader = Region::new(0x44000000, 0x300000).unwrap();
    for (minimum, expected) in [
        (0, 0x40200000),
        (0x41000000, 0x41000000),
        (0x44000000, 0x44400000),
    ] {
        let plan = LoadPlan::new(images(&kernel, &root, &dtb), loader, minimum).unwrap();
        assert_eq!(plan.kernel.physical().start(), expected);
        assert_eq!(plan.kernel.virtual_start(), platform::KERNEL_VA_START);
        assert_eq!(plan.dtb.start(), expected + 4096);
        assert_eq!(plan.root.physical().start(), expected + 8192);
        assert_eq!(plan.headers.start(), plan.root.physical().end());
        assert_eq!(plan.headers.size(), PAGE);
    }
    assert!(matches!(
        LoadPlan::new(images(&kernel, &root, &dtb), loader, platform::RAM_END),
        Err(BootError::Layout(layout::Error::NoMemory))
    ));
}
#[test]
fn planner_rejects_invalid_address_windows() {
    let kernel = elf(0x400000, false);
    let root = elf(0x10000, true);
    let dtb = dtb();
    let loader = Region::new(0x44000000, 0x100000).unwrap();
    assert!(matches!(
        LoadPlan::new(images(&kernel, &root, &dtb), loader, 0),
        Err(BootError::Layout(layout::Error::KernelWindow))
    ));
    let kernel = elf(platform::KERNEL_VA_START, false);
    let root = elf(0, false);
    assert!(matches!(
        LoadPlan::new(images(&kernel, &root, &dtb), loader, 0),
        Err(BootError::Layout(layout::Error::RootLayout))
    ));
}
#[test]
fn allocator_handles_unsorted_overlapping_reservations_and_exact_fit() {
    let ram = Region::new(0x1000, 0x9000).unwrap();
    let reserved = [
        Region::new(0x4000, 0x1000).unwrap(),
        Region::new(0x1000, 0x4000).unwrap(),
    ];
    let result = layout::allocate(ram, &reserved, 0, 0x5000, 0x1000).unwrap();
    assert_eq!((result.start(), result.end()), (0x5000, 0xa000));
    assert!(matches!(
        layout::allocate(ram, &reserved, 0, 0x5001, 0x1000),
        Err(layout::Error::NoMemory)
    ));
    assert!(matches!(
        Region::new(usize::MAX, 1),
        Err(layout::Error::Overflow)
    ));
}
#[test]
fn device_tree_view_checks_bounds_and_limits_to_declared_size() {
    let mut bytes = dtb();
    for len in 0..40 {
        assert!(DeviceTree::parse(&bytes[..len]).is_err());
    }
    bytes.extend_from_slice(b"padding");
    assert_eq!(DeviceTree::parse(&bytes).unwrap().bytes().len(), 40);
    bytes[4..8].copy_from_slice(&48u32.to_be_bytes());
    assert!(DeviceTree::parse(&bytes).is_err());
}

#[test]
fn root_layout_follows_elf_without_a_kernel_stack_contract() {
    let kernel = elf(platform::KERNEL_VA_START, false);
    let dtb = dtb();
    let loader = Region::new(0x44000000, 0x100000).unwrap();
    for base in [0x10000, 0x200000, 0x3000000] {
        for with_stack in [false, true] {
            let root = elf(base, with_stack);
            let plan = LoadPlan::new(images(&kernel, &root, &dtb), loader, 0).unwrap();
            assert_eq!(plan.root.virtual_start(), base);
        }
    }
    for image in [
        0..4096,
        4097..8192,
        4096..4096,
        4096..0x800000,
        0x7fff000..0x8000000,
        (usize::MAX - 4095) as u64..u64::MAX,
    ] {
        assert!(InitialTaskLayout::new(image, 40).is_none());
    }
    let layout = InitialTaskLayout::new(0x10000..0x17000, 4096).unwrap();
    assert_eq!(layout.ipc_buffer, 0x17000);
    assert_eq!(layout.boot_info, 0x18000);
    assert_eq!(layout.extra, 0x19000);
    assert_eq!(layout.end, 0x1b000);
}
