//! The user-protocol label segments must be disjoint (docs/service-manager.md,
//! decision 14): a service that handles two protocols on one endpoint would
//! otherwise shadow one of them. Mirrors seL4's contiguous per-object-type
//! invocation ranges.
use rstiny_protocol::{LABELS, SEGMENT_SIZE, SEGMENTS};

#[test]
fn segments_are_disjoint() {
    let mut bases: Vec<u64> = SEGMENTS.iter().map(|(_, base)| *base).collect();
    bases.sort_unstable();
    for window in bases.windows(2) {
        assert!(
            window[1] - window[0] >= SEGMENT_SIZE,
            "segments overlap: {:#x} and {:#x}",
            window[0],
            window[1]
        );
    }
}

#[test]
fn every_label_stays_inside_its_segment_and_is_unique() {
    let mut seen = std::collections::BTreeSet::new();
    for (name, label, base) in LABELS {
        assert!(
            *label >= *base && *label < base + SEGMENT_SIZE,
            "{name} = {label:#x} is outside [{base:#x}, {:#x})",
            base + SEGMENT_SIZE
        );
        assert!(seen.insert(*label), "duplicate label {label:#x} ({name})");
    }
}

#[test]
fn runtime_extension_segment_is_separate() {
    // RuntimeInvocation owns 0x1000..=0x10ff; user segments stay below it.
    assert!(rstiny_protocol::fs::BASE + SEGMENT_SIZE <= 0x1000);
}
