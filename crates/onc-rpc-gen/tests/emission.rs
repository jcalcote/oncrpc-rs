use onc_rpcgen::{
    LoadOptions, emit_rust_types, fixture_root, generate_from_x_file_with_options, parse_x_file,
    parse_x_source,
};
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

fn generated_module_fixture(path: &str, module: &str) -> String {
    generate_from_x_file_with_options(
        fixture(path),
        &LoadOptions {
            include_dirs: vec![fixture("xdr/real")],
        },
        &Default::default(),
    )
    .expect("fixture should generate")
    .modules
    .into_iter()
    .find(|output| output.module_name == module)
    .and_then(|output| output.types)
    .expect("requested generated module should exist")
}

#[test]
fn emission_snapshot_matches_expected_output_for_real_fixtures() {
    assert_snapshot(
        "xdr/real/storage_types_basic.x",
        "expected/storage_types_basic.rs.txt",
    );
    assert_snapshot(
        "xdr/synthetic/rfc4506_parser_features.x",
        "expected/rfc4506_parser_features.rs.txt",
    );
}

#[test]
fn emission_maps_real_subset_to_expected_rust_shapes() {
    let emitted = emitted_fixture("xdr/real/storage_types_basic.x");

    assert!(emitted.contains("use bytes::Bytes;"));
    assert!(emitted.contains("pub const HANDLE_SIZE: i64 = 64;"));
    assert!(emitted.contains("pub type trace_id_t = u64;"));
    assert!(emitted.contains("pub type file_handle_t = Bytes;"));
    assert!(emitted.contains("pub type path_t = String;"));
    assert!(emitted.contains("pub struct timestamp_t {"));
    assert!(emitted.contains("pub seconds: u32,"));
    assert!(emitted.contains("#[repr(i32)]"));
    assert!(emitted.contains("pub enum status_t {"));
}

#[test]
fn emission_is_deterministic_for_same_schema() {
    let first = generated_module_fixture("xdr/real/blob_service_basic.x", "blob_service_basic");
    let second = generated_module_fixture("xdr/real/blob_service_basic.x", "blob_service_basic");

    assert_eq!(first, second);
}

#[test]
fn emission_includes_program_version_and_procedure_constants() {
    let emitted = generated_module_fixture("xdr/real/blob_service_basic.x", "blob_service_basic");

    assert!(emitted.contains("pub mod blob_service {"));
    assert!(emitted.contains("pub const PROGRAM: u32 = 200001;"));
    assert!(emitted.contains("pub mod blob_service_v1 {"));
    assert!(emitted.contains("pub const VERSION: u32 = 1;"));
    assert!(emitted.contains("pub const BLOB_NULL: u32 = 0;"));
    assert!(emitted.contains("pub const BLOB_COPY: u32 = 1;"));
}

#[test]
fn emission_covers_remaining_rfc4506_constructs() {
    let emitted = emitted_fixture("xdr/synthetic/rfc4506_parser_features.x");

    assert!(emitted.contains("pub type ratio_t = f32;"));
    assert!(emitted.contains("pub type measure_t = f64;"));
    assert!(emitted.contains("pub type wide_t = [u8; 16];"));
    assert!(emitted.contains("pub fixed: [u8; OCTAL_BOUND as usize],"));
    assert!(emitted.contains("pub next: Option<Box<ratio_t>>,"));
    assert!(emitted.contains("pub enum payload_t {"));
    assert!(emitted.contains("Case1 {"));
    assert!(emitted.contains("Case2 {"));
    assert!(emitted.contains("Case3 {"));
    assert!(emitted.contains("Default {"));
    assert!(emitted.contains("pub enum enum_payload_t {"));
    assert!(emitted.contains("discriminant: i32,"));
    assert!(emitted.contains("let discriminant = <i32 as XdrDecode>::decode_xdr(input)?;"));
    assert!(emitted.contains("value if value == mode_t::MODE_A as i32 => Ok(Self::ModeA {"));
    assert!(emitted.contains("pub enum inline_enum_t {"));
    assert!(emitted.contains("impl XdrEncode for payload_t {"));
    assert!(emitted.contains("impl XdrDecode for payload_t {"));
}

#[test]
fn emission_generates_serializers_for_real_and_cross_module_types() {
    let emitted = emitted_fixture("xdr/real/storage_types_basic.x");
    let blob = generated_module_fixture("xdr/real/blob_service_basic.x", "blob_service_basic");

    assert!(emitted.contains("use onc_rpc_xdr::{XdrDecode, XdrEncode};"));
    assert!(emitted.contains("impl XdrEncode for timestamp_t {"));
    assert!(emitted.contains("impl XdrDecode for status_t {"));

    assert!(blob.contains("impl XdrEncode for copy_request_t {"));
    assert!(blob.contains("crate::common_types::job_id_t"));
    assert!(blob.contains("crate::transfer_types::instance_info_t"));
}

#[test]
fn emission_escapes_rust_keywords_and_qualifies_union_discriminants() {
    let source = r#"
enum mode_t {
    MODE_A = 0,
    MODE_B = 1
};

struct keyword_t {
    int type;
    int where;
};

union bool_union_t switch (bool enabled) {
case TRUE:
    int value;
case FALSE:
    void;
};

union enum_union_t switch (mode_t mode) {
case MODE_A:
    int value;
default:
    void;
};
"#;

    let schema = parse_x_source(source).expect("source should parse");
    let emitted = emit_rust_types(&schema).expect("source should emit");

    assert!(emitted.contains("pub r#type: i32,"));
    assert!(emitted.contains("pub r#where: i32,"));
    assert!(emitted.contains("self.r#type.encode_xdr(output)?;"));
    assert!(emitted.contains("self.r#where.encode_xdr(output)?;"));
    assert!(emitted.contains("true => Ok(Self::True {"));
    assert!(emitted.contains("false => Ok(Self::False)"));
    assert!(emitted.contains("Self::Default { discriminant } => {"));
    assert!(emitted.contains("let discriminant = <i32 as XdrDecode>::decode_xdr(input)?;"));
    assert!(emitted.contains("value if value == mode_t::MODE_A as i32 => Ok(Self::ModeA {"));
}

fn assert_snapshot(fixture_path: &str, expected_path: &str) {
    let actual = emitted_fixture(fixture_path);
    let expected =
        fs::read_to_string(fixture(expected_path)).expect("expected snapshot should exist");

    assert_eq!(actual, expected);
}
