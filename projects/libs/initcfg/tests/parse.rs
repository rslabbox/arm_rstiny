use rstiny_initcfg::{Restart, parse};

const SAMPLE: &str = r#"
# init.cfg - service manifest
service console {
    elf = "console.elf"
    restart = always
    max_restarts = 5
    window_ms = 60000
    backoff_ms = 100
}

service block {
    elf = "block.elf"      # inline comment
    depends = console
    restart = on-failure
    device = virtio-mmio-0
    budget = 2M
}
"#;

#[test]
fn parses_the_manifest_sample() {
    let config = parse(SAMPLE).unwrap();
    assert_eq!(config.names(), ["console", "block"]);
    let console = &config.services[0];
    assert_eq!(console.elf, "console.elf");
    assert_eq!(console.restart, Restart::Always);
    assert_eq!(console.max_restarts, 5);
    assert_eq!(console.backoff_ms, 100);
    let block = &config.services[1];
    assert_eq!(block.depends, ["console"]);
    assert_eq!(block.devices, ["virtio-mmio-0"]);
    assert_eq!(block.budget_bits, 21); // 2M
}

#[test]
fn rejects_malformed_manifests() {
    for text in [
        "service console {\n",                 // unterminated
        "service console {\n  unknown = 1\n}", // unknown key
        "service console {}\n",                // missing elf
        "service console {\n elf = \"a\"\n}\nservice console {\n elf = \"a\"\n}", // duplicate
        "service console {\n depends = missing\n elf = \"a\"\n}", // dangling dependency
        "service a {\n depends = b\n elf = \"a\"\n}\nservice b {\n depends = a\n elf = \"b\"\n}", // cycle
        "service console {\n restart = sometimes\n elf = \"c\"\n}", // bad enum
        "service console {\n budget = 3M\n elf = \"c\"\n}",         // not a power of two
    ] {
        assert!(parse(text).is_err(), "accepted: {text}");
    }
}
