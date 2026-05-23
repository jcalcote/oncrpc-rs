use onc_rpcgen::{emit_rust_types, fixture_root, parse_x_file};
use std::fs;
use std::path::{Path, PathBuf};

fn fixture(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(fixture_root())
        .join(path)
}

fn emitted_fixture(path: &str) -> String {
    let schema = parse_x_file(fixture(path)).expect("fixture should parse");
    emit_rust_types(&schema).expect("fixture should emit")
}

#[test]
fn emission_snapshot_matches_expected_output_for_real_fixtures() {
    assert_snapshot(
        "xdr/real/pd_types_basic.x",
        "expected/pd_types_basic.rs.txt",
    );
    assert_snapshot(
        "xdr/real/pdcm_program_basic.x",
        "expected/pdcm_program_basic.rs.txt",
    );
}

#[test]
fn emission_maps_real_subset_to_expected_rust_shapes() {
    let emitted = emitted_fixture("xdr/real/pd_types_basic.x");

    assert!(emitted.contains("use bytes::Bytes;"));
    assert!(emitted.contains("pub const FILE_HANDLE_LEN: i64 = 64;"));
    assert!(emitted.contains("pub type pdx_trc_id_t = u64;"));
    assert!(emitted.contains("pub type pdx_file_handle_t = Bytes;"));
    assert!(emitted.contains("pub type pdx_path_t = String;"));
    assert!(emitted.contains("pub struct pdx_time_t {"));
    assert!(emitted.contains("pub seconds: u32,"));
    assert!(emitted.contains("#[repr(i32)]"));
    assert!(emitted.contains("pub enum pdx_status_t {"));
}

#[test]
fn emission_is_deterministic_for_same_schema() {
    let first = emitted_fixture("xdr/real/pdcm_program_basic.x");
    let second = emitted_fixture("xdr/real/pdcm_program_basic.x");

    assert_eq!(first, second);
}

#[test]
fn emission_includes_program_version_and_procedure_constants() {
    let emitted = emitted_fixture("xdr/real/pdcm_program_basic.x");

    assert!(emitted.contains("pub mod pdcm_program {"));
    assert!(emitted.contains("pub const PROGRAM: u32 = 100666;"));
    assert!(emitted.contains("pub mod pdcm_rpc_v8 {"));
    assert!(emitted.contains("pub const VERSION: u32 = 8;"));
    assert!(emitted.contains("pub const PDCM_NULL: u32 = 0;"));
    assert!(emitted.contains("pub const PDCM_DO_COPY: u32 = 1;"));
}

fn assert_snapshot(fixture_path: &str, expected_path: &str) {
    let actual = emitted_fixture(fixture_path);
    let expected =
        fs::read_to_string(fixture(expected_path)).expect("expected snapshot should exist");

    assert_eq!(actual, expected);
}
