use onc_rpcgen::{emit_rust_stubs, fixture_root, parse_x_file};
use std::fs;
use std::path::{Path, PathBuf};

fn fixture(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(fixture_root())
        .join(path)
}

fn emitted_fixture(path: &str) -> String {
    let schema = parse_x_file(fixture(path)).expect("fixture should parse");
    emit_rust_stubs(&schema).expect("fixture should emit")
}

#[test]
fn stub_snapshot_matches_expected_output_for_real_fixture() {
    assert_snapshot(
        "xdr/real/pdcm_program_basic.x",
        "expected/pdcm_program_basic.stubs.rs.txt",
    );
}

#[test]
fn stub_emission_is_deterministic() {
    let first = emitted_fixture("xdr/real/pdcm_program_basic.x");
    let second = emitted_fixture("xdr/real/pdcm_program_basic.x");

    assert_eq!(first, second);
}

#[test]
fn stub_emission_generates_client_service_and_dispatch_shapes() {
    let emitted = emitted_fixture("xdr/real/pdcm_program_basic.x");

    assert!(emitted.contains("pub struct PDCM_RPC_V8Client<T> {"));
    assert!(emitted.contains("pub fn pdcm_null(&self) -> Result<onc_rpc_runtime::CallResponse, onc_rpc_runtime::RuntimeError> {"));
    assert!(emitted.contains("pub fn pdcm_do_copy(&self, payload: Bytes) -> Result<onc_rpc_runtime::CallResponse, onc_rpc_runtime::RuntimeError> {"));
    assert!(emitted.contains("pub trait PDCM_RPC_V8Service {"));
    assert!(
        emitted.contains("fn pdcm_null(&self) -> Result<Bytes, onc_rpc_server::DispatchError>;")
    );
    assert!(emitted.contains(
        "fn pdcm_do_copy(&self, payload: Bytes) -> Result<Bytes, onc_rpc_server::DispatchError>;"
    ));
    assert!(emitted.contains("pub struct PDCM_RPC_V8Dispatch<T> {"));
    assert!(emitted.contains("impl<T> onc_rpc_server::Dispatch for PDCM_RPC_V8Dispatch<T>"));
    assert!(emitted.contains("_ => Err(onc_rpc_server::DispatchError::ProcedureUnavailable),"));
}

#[test]
fn stub_emission_is_empty_for_schema_without_programs() {
    let emitted = emitted_fixture("xdr/real/pd_types_basic.x");

    assert!(emitted.is_empty());
}

fn assert_snapshot(fixture_path: &str, expected_path: &str) {
    let actual = emitted_fixture(fixture_path);
    let expected =
        fs::read_to_string(fixture(expected_path)).expect("expected snapshot should exist");

    assert_eq!(actual, expected);
}
