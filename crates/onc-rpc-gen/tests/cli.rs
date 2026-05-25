use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn fixture_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(onc_rpcgen::fixture_root())
        .to_path_buf()
}

#[test]
fn help_option_prints_usage() {
    Command::cargo_bin("onc-rpcgen")
        .expect("cli binary should build")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage: onc-rpcgen"))
        .stdout(predicate::str::contains("parse"))
        .stdout(predicate::str::contains("generate"));
}

#[test]
fn parse_command_can_emit_ast() {
    let fixture = fixture_root().join("xdr/real/storage_types_basic.x");

    Command::cargo_bin("onc-rpcgen")
        .expect("cli binary should build")
        .args(["parse", "--emit-ast"])
        .arg(fixture)
        .assert()
        .success()
        .stdout(predicate::str::contains("struct timestamp_t"))
        .stdout(predicate::str::contains("enum status_t"));
}

#[test]
fn generate_command_writes_types_and_stubs_files() {
    let temp = TempDir::new().expect("tempdir should exist");
    let fixture = fixture_root().join("xdr/real/blob_service_basic.x");

    Command::cargo_bin("onc-rpcgen")
        .expect("cli binary should build")
        .args(["generate", "--out-dir"])
        .arg(temp.path())
        .arg(fixture)
        .assert()
        .success();

    let types = fs::read_to_string(temp.path().join("blob_service_basic.types.rs"))
        .expect("types output should exist");
    let stubs = fs::read_to_string(temp.path().join("blob_service_basic.stubs.rs"))
        .expect("stubs output should exist");
    let transfer_types = fs::read_to_string(temp.path().join("transfer_types.types.rs"))
        .expect("transitive types output should exist");

    assert!(types.contains("pub mod blob_service {"));
    assert!(types.contains("crate::common_types::job_id_t"));
    assert!(types.contains("crate::transfer_types::job_update_t"));
    assert!(stubs.contains("pub mod client {"));
    assert!(stubs.contains("pub mod async_client {"));
    assert!(stubs.contains("pub mod server {"));
    assert!(stubs.contains("pub mod async_server {"));
    assert!(stubs.contains("pub struct BLOB_SERVICE_V1Client<T> {"));
    assert!(!stubs.contains("nfs_support_program"));
    assert!(transfer_types.contains("crate::nfs_support::remote_handle_t"));
    assert!(!temp.path().join("nfs_support.stubs.rs").exists());
}

#[test]
fn generate_command_resolves_include_dirs() {
    let temp = TempDir::new().expect("tempdir should exist");
    let schema_dir = temp.path().join("schema");
    fs::create_dir_all(&schema_dir).expect("schema dir should exist");

    fs::write(
        schema_dir.join("shared_types.x"),
        "typedef unsigned hyper handle_id_t;\n",
    )
    .expect("include fixture should be written");
    fs::write(
        schema_dir.join("service.x"),
        "%#include \"demo/shared_types.h\"\n\
         struct sample_t { handle_id_t id; };\n\
         program SAMPLE_PROG { version SAMPLE_V1 { void PING(void) = 0; } = 1; } = 99;\n",
    )
    .expect("service fixture should be written");

    let out_dir = temp.path().join("out");

    Command::cargo_bin("onc-rpcgen")
        .expect("cli binary should build")
        .args(["generate", "--include-dir"])
        .arg(&schema_dir)
        .args(["--out-dir"])
        .arg(&out_dir)
        .arg(schema_dir.join("service.x"))
        .assert()
        .success();

    let types =
        fs::read_to_string(out_dir.join("service.types.rs")).expect("types output should exist");
    let shared_types = fs::read_to_string(out_dir.join("shared_types.types.rs"))
        .expect("shared types output should exist");
    let stubs =
        fs::read_to_string(out_dir.join("service.stubs.rs")).expect("stubs output should exist");

    assert!(shared_types.contains("pub type handle_id_t = u64;"));
    assert!(types.contains("pub struct sample_t {"));
    assert!(types.contains("crate::shared_types::handle_id_t"));
    assert!(stubs.contains("pub struct SAMPLE_V1Client<T> {"));
    assert!(!out_dir.join("shared_types.stubs.rs").exists());
}

#[test]
fn emit_stubs_command_can_omit_client_or_server_sections() {
    let temp = TempDir::new().expect("tempdir should exist");
    let fixture = fixture_root().join("xdr/real/blob_service_basic.x");

    Command::cargo_bin("onc-rpcgen")
        .expect("cli binary should build")
        .args(["emit", "stubs", "--no-client", "--out-dir"])
        .arg(temp.path())
        .arg(&fixture)
        .assert()
        .success();

    let stubs = fs::read_to_string(temp.path().join("blob_service_basic.stubs.rs"))
        .expect("stubs output should exist");
    assert!(stubs.contains("pub mod server {"));
    assert!(stubs.contains("pub mod async_server {"));
    assert!(!stubs.contains("pub struct BLOB_SERVICE_V1Client<T>"));
    assert!(!stubs.contains("pub mod async_client {"));
    assert!(stubs.contains("pub trait BLOB_SERVICE_V1Service {"));

    let temp = TempDir::new().expect("tempdir should exist");
    Command::cargo_bin("onc-rpcgen")
        .expect("cli binary should build")
        .args(["emit", "stubs", "--no-server", "--out-dir"])
        .arg(temp.path())
        .arg(&fixture)
        .assert()
        .success();

    let stubs = fs::read_to_string(temp.path().join("blob_service_basic.stubs.rs"))
        .expect("stubs output should exist");
    assert!(stubs.contains("pub mod client {"));
    assert!(stubs.contains("pub mod async_client {"));
    assert!(stubs.contains("pub struct BLOB_SERVICE_V1Client<T>"));
    assert!(!stubs.contains("pub trait BLOB_SERVICE_V1Service {"));
    assert!(!stubs.contains("pub mod async_server {"));
}

#[test]
fn generate_command_verbose_lists_written_files() {
    let temp = TempDir::new().expect("tempdir should exist");
    let fixture = fixture_root().join("xdr/real/blob_service_basic.x");

    Command::cargo_bin("onc-rpcgen")
        .expect("cli binary should build")
        .args(["generate", "--verbose", "--out-dir"])
        .arg(temp.path())
        .arg(fixture)
        .assert()
        .success()
        .stdout(predicate::str::contains("blob_service_basic.types.rs"))
        .stdout(predicate::str::contains("blob_service_basic.stubs.rs"))
        .stdout(predicate::str::contains("transfer_types.types.rs"));
}
