use crate::support::xeibe_ok;

#[test]
fn version_lists_build_facts() {
    for flag in ["-V", "--version"] {
        check(&xeibe_ok(&[flag]).stdout);
    }
}

fn check(stdout: &str) {
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[0], concat!("xeibe ", env!("CARGO_PKG_VERSION")));
    assert!(lines[1].starts_with("EPSG dataset "), "{stdout}");
    assert!(lines[2].starts_with("features: "), "{stdout}");
    assert!(lines[3].starts_with("arrow/parquet "), "{stdout}");
}
