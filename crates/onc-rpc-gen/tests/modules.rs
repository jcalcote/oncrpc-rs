use onc_rpcgen::{
    GeneratedModuleOutput, LoadOptions, fixture_root, generate_from_x_file_with_options,
    load_module_set_from_x_file_with_options,
};
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

fn generated_fixture(path: &str) -> onc_rpcgen::GeneratedOutputs {
    generate_from_x_file_with_options(
        fixture(path),
        &LoadOptions {
            include_dirs: vec![include_dir()],
        },
        &Default::default(),
    )
    .expect("fixture should generate")
}

fn module_output<'a>(
    outputs: &'a [GeneratedModuleOutput],
    name: &str,
) -> &'a GeneratedModuleOutput {
    outputs
        .iter()
        .find(|output| output.module_name == name)
        .expect("module output should exist")
}

#[test]
fn module_loading_tracks_real_dependency_chain() {
    let loaded = load_module_set_from_x_file_with_options(
        &fixture("xdr/real/blob_service_basic.x"),
        &LoadOptions {
            include_dirs: vec![include_dir()],
        },
    )
    .expect("fixture should load");

    let module_names = loaded
        .modules
        .iter()
        .map(|module| module.module_name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        module_names,
        vec![
            "common_types",
            "nfs_support",
            "transfer_types",
            "blob_service_basic",
        ]
    );

    let root = loaded
        .modules
        .iter()
        .find(|module| module.module_name == "blob_service_basic")
        .expect("root module should exist");
    assert_eq!(root.dependencies, vec!["common_types", "transfer_types"]);

    let transfer = loaded
        .modules
        .iter()
        .find(|module| module.module_name == "transfer_types")
        .expect("transfer_types should exist");
    assert_eq!(transfer.dependencies, vec!["common_types", "nfs_support"]);
}

#[test]
fn module_generation_keeps_transitive_programs_in_their_own_modules() {
    let outputs = generated_fixture("xdr/real/blob_service_basic.x");

    let root_types = module_output(&outputs.modules, "blob_service_basic")
        .types
        .as_ref()
        .expect("root types should exist");
    let root_stubs = module_output(&outputs.modules, "blob_service_basic")
        .stubs
        .as_ref()
        .expect("root stubs should exist");
    let transfer_types = module_output(&outputs.modules, "transfer_types")
        .types
        .as_ref()
        .expect("transfer types should exist");
    let nfs_module = module_output(&outputs.modules, "nfs_support");

    assert_eq!(
        root_types.trim_end(),
        fs::read_to_string(fixture(
            "expected/blob_service_basic.generated.types.rs.txt"
        ))
        .expect("expected root types snapshot should exist")
        .trim_end(),
    );
    assert_eq!(
        root_stubs.trim_end(),
        fs::read_to_string(fixture(
            "expected/blob_service_basic.generated.stubs.rs.txt"
        ))
        .expect("expected root stubs snapshot should exist")
        .trim_end(),
    );
    assert_eq!(
        transfer_types.trim_end(),
        fs::read_to_string(fixture("expected/transfer_types.generated.types.rs.txt"))
            .expect("expected transfer types snapshot should exist")
            .trim_end(),
    );
    assert!(root_types.contains("crate::common_types::job_id_t"));
    assert!(root_types.contains("crate::transfer_types::job_update_t"));
    assert!(!root_stubs.contains("nfs_support_program"));
    assert!(nfs_module.stubs.is_none());
}
