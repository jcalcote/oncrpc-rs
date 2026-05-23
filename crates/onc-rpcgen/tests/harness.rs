use onc_rpcgen::{GeneratorError, fixture_root, generate_from_x_file};
use std::path::Path;

fn fixture(path: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(fixture_root())
        .join(path)
        .display()
        .to_string()
}

#[test]
fn fixture_layout_exists() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(fixture_root());

    assert!(root.join("xdr/real").is_dir());
    assert!(root.join("xdr/synthetic").is_dir());
    assert!(root.join("expected").is_dir());
}

#[test]
fn generator_errors_include_fixture_path_until_implemented() {
    let fixture_path = fixture("xdr/synthetic/not-yet-implemented.x");

    let error =
        generate_from_x_file(&fixture_path).expect_err("generator should still be scaffold-only");

    match error {
        GeneratorError::NotImplemented(path) => assert_eq!(path, fixture_path),
    }
}
