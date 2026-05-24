use onc_rpcgen::{GenerateOptions, LoadOptions, generate_from_x_file_with_options};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

fn datasphere_xdr_dir() -> PathBuf {
    Path::new("/home/jcalcote/dev/git/datasphere/pd-shared-api/xdr").to_path_buf()
}

#[test]
fn datasphere_corpus_generates_and_compiles_when_available() {
    let xdr_dir = datasphere_xdr_dir();
    if !xdr_dir.is_dir() {
        eprintln!(
            "skipping datasphere corpus test; {} not present",
            xdr_dir.display()
        );
        return;
    }

    let out = TempDir::new().expect("tempdir should exist");
    let out_dir = out.path().join("generated");
    fs::create_dir_all(&out_dir).expect("generated dir should exist");
    let load = LoadOptions {
        include_dirs: vec![xdr_dir.clone()],
    };

    for entry in fs::read_dir(&xdr_dir).expect("xdr dir should be readable") {
        let path = entry.expect("dir entry should exist").path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("x") {
            continue;
        }
        let outputs = generate_from_x_file_with_options(&path, &load, &GenerateOptions::default())
            .expect("real corpus file should generate");
        for module in outputs.modules {
            if let Some(types) = module.types {
                fs::write(
                    out_dir.join(format!("{}.types.rs", module.module_name)),
                    types,
                )
                .expect("types output should write");
            }
            if let Some(stubs) = module.stubs {
                fs::write(
                    out_dir.join(format!("{}.stubs.rs", module.module_name)),
                    stubs,
                )
                .expect("stubs output should write");
            }
        }
    }

    let mut lib_rs = String::new();
    let mut entries = fs::read_dir(&out_dir)
        .expect("generated dir should exist")
        .map(|entry| entry.expect("dir entry should exist").path())
        .collect::<Vec<_>>();
    entries.sort();

    for path in entries {
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("generated filename should be valid utf-8");
        if let Some(module) = filename.strip_suffix(".types.rs") {
            lib_rs.push_str(&format!(
                "pub mod {module} {{ include!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/generated/{filename}\")); }}\n"
            ));
        } else if let Some(module) = filename.strip_suffix(".stubs.rs") {
            lib_rs.push_str(&format!(
                "pub mod {module}_stubs {{ include!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/generated/{filename}\")); }}\n"
            ));
        }
    }

    fs::create_dir_all(out.path().join("src")).expect("src dir should exist");
    fs::write(out.path().join("src/lib.rs"), lib_rs).expect("lib.rs should write");
    fs::write(
        out.path().join("Cargo.toml"),
        format!(
            "[package]\nname = \"generated-corpus-check\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nbytes = \"1\"\nonc-rpc-runtime = {{ path = \"{}/crates/onc-rpc-runtime\" }}\nonc-rpc-server = {{ path = \"{}/crates/onc-rpc-server\" }}\nonc-rpc-xdr = {{ path = \"{}/crates/onc-rpc-xdr\" }}\n",
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .and_then(Path::parent)
                .expect("repo root should exist")
                .display(),
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .and_then(Path::parent)
                .expect("repo root should exist")
                .display(),
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .and_then(Path::parent)
                .expect("repo root should exist")
                .display(),
        ),
    )
    .expect("Cargo.toml should write");

    let status = Command::new("cargo")
        .arg("check")
        .arg("--manifest-path")
        .arg(out.path().join("Cargo.toml"))
        .status()
        .expect("cargo check should run");

    assert!(status.success(), "generated corpus crate should compile");
}
