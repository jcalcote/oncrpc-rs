use onc_rpcgen::{LoadOptions, fixture_root, generate_from_x_file_with_options};
use std::fs;
use std::path::{Path, PathBuf};

fn fixture(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(fixture_root())
        .join(path)
}

fn include_dir() -> PathBuf {
    fixture("xdr/real")
}

fn emitted_fixture(path: &str) -> String {
    let outputs = generate_from_x_file_with_options(
        fixture(path),
        &LoadOptions {
            include_dirs: vec![include_dir()],
        },
        &Default::default(),
    )
    .expect("fixture should generate");

    outputs
        .modules
        .into_iter()
        .find(|module| module.module_name == outputs.root_module)
        .and_then(|module| module.stubs)
        .unwrap_or_default()
}

#[test]
fn stub_snapshot_matches_expected_output_for_real_fixture() {
    assert_snapshot(
        "xdr/real/blob_service_basic.x",
        "expected/blob_service_basic.generated.stubs.rs.txt",
    );
}

#[test]
fn stub_emission_is_deterministic() {
    let first = emitted_fixture("xdr/real/blob_service_basic.x");
    let second = emitted_fixture("xdr/real/blob_service_basic.x");

    assert_eq!(first, second);
}

#[test]
fn stub_emission_generates_client_service_and_dispatch_shapes() {
    let emitted = emitted_fixture("xdr/real/blob_service_basic.x");

    assert!(emitted.contains("pub mod client {"));
    assert!(emitted.contains("pub struct BLOB_SERVICE_V1Client<T> {"));
    assert!(
        emitted.contains("pub fn blob_null(&self) -> Result<(), onc_rpc_runtime::RuntimeError> {")
    );
    assert!(emitted.contains(
        "pub fn blob_null_with_options(&self, options: &onc_rpc_runtime::CallOptions) -> Result<(), onc_rpc_runtime::RuntimeError> {"
    ));
    assert!(emitted.contains(
        "pub fn blob_copy(&self, argument: crate::blob_service_basic::copy_request_t) -> Result<crate::transfer_types::job_result_t, onc_rpc_runtime::RuntimeError> {"
    ));
    assert!(emitted.contains(
        "pub fn blob_copy_with_options(&self, argument: crate::blob_service_basic::copy_request_t, options: &onc_rpc_runtime::CallOptions) -> Result<crate::transfer_types::job_result_t, onc_rpc_runtime::RuntimeError> {"
    ));
    assert!(emitted.contains("pub mod server {"));
    assert!(emitted.contains("pub trait BLOB_SERVICE_V1Service {"));
    assert!(emitted.contains("fn blob_null(&self, request: &onc_rpc_server::RequestContext) -> Result<(), onc_rpc_server::DispatchError>;"));
    assert!(emitted.contains(
        "fn blob_copy(&self, request: &onc_rpc_server::RequestContext, argument: crate::blob_service_basic::copy_request_t) -> Result<crate::transfer_types::job_result_t, onc_rpc_server::DispatchError>;"
    ));
    assert!(emitted.contains("pub struct BLOB_SERVICE_V1Dispatch<T> {"));
    assert!(emitted.contains("impl<T> onc_rpc_server::Dispatch for BLOB_SERVICE_V1Dispatch<T>"));
    assert!(emitted.contains("pub mod async_client {"));
    assert!(emitted.contains(
        "pub async fn blob_copy(&self, argument: crate::blob_service_basic::copy_request_t) -> Result<crate::transfer_types::job_result_t, onc_rpc_runtime::RuntimeError> {"
    ));
    assert!(emitted.contains(
        "pub async fn blob_copy_with_options(&self, argument: crate::blob_service_basic::copy_request_t, options: &onc_rpc_runtime::CallOptions) -> Result<crate::transfer_types::job_result_t, onc_rpc_runtime::RuntimeError> {"
    ));
    assert!(emitted.contains("pub mod async_server {"));
    assert!(emitted.contains("#[onc_rpc_server::async_trait]"));
    assert!(emitted.contains(
        "async fn blob_copy(&self, request: &onc_rpc_server::RequestContext, argument: crate::blob_service_basic::copy_request_t) -> Result<crate::transfer_types::job_result_t, onc_rpc_server::DispatchError>;"
    ));
    assert!(
        emitted.contains("impl<T> onc_rpc_server::AsyncDispatch for BLOB_SERVICE_V1Dispatch<T>")
    );
    assert!(emitted.contains("let response = self.inner.blob_copy(&request, argument).await?;"));
    assert!(emitted.contains(
        "<crate::blob_service_basic::copy_request_t as onc_rpc_xdr::XdrDecode>::from_xdr_bytes(&request.payload)"
    ));
    assert!(emitted.contains("let payload = onc_rpc_xdr::XdrEncode::to_xdr_bytes(&response)"));
    assert!(emitted.contains("_ => Err(onc_rpc_server::DispatchError::ProcedureUnavailable),"));
}

#[test]
fn stub_emission_is_empty_for_schema_without_programs() {
    let emitted = emitted_fixture("xdr/real/storage_types_basic.x");

    assert!(emitted.is_empty());
}

fn assert_snapshot(fixture_path: &str, expected_path: &str) {
    let actual = emitted_fixture(fixture_path);
    let expected =
        fs::read_to_string(fixture(expected_path)).expect("expected snapshot should exist");

    assert_eq!(actual, expected);
}
