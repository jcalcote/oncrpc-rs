use onc_rpcgen::{
    DeclaratorModifier, GeneratorError, Item, Schema, TypeSpec, ValueExpr, fixture_root,
    parse_x_file, parse_x_source,
};
use std::fs;
use std::path::{Path, PathBuf};

fn fixture(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(fixture_root())
        .join(path)
}

fn read_schema(path: &str) -> Schema {
    parse_x_file(fixture(path)).expect("fixture should parse")
}

#[test]
fn fixture_layout_exists() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(fixture_root());

    assert!(root.join("xdr/real").is_dir());
    assert!(root.join("xdr/synthetic").is_dir());
    assert!(root.join("expected").is_dir());
}

#[test]
fn parses_real_pd_types_subset_fixture() {
    let schema = read_schema("xdr/real/pd_types_basic.x");

    assert!(matches!(&schema.items[0], Item::Const(item) if item.name == "FILE_HANDLE_LEN"));
    assert!(
        matches!(&schema.items[1], Item::Typedef(item) if item.target == TypeSpec::UnsignedHyper)
    );
    assert!(matches!(
        &schema.items[2],
        Item::Typedef(item)
            if item.target == TypeSpec::Opaque
                && item.declarator.modifier
                    == Some(DeclaratorModifier::VariableArray(Some(ValueExpr::Identifier(
                        "FILE_HANDLE_LEN".to_string()
                    ))))
    ));
    assert!(matches!(&schema.items[4], Item::Struct(item) if item.name == "pdx_time_t"));
    assert!(matches!(&schema.items[5], Item::Enum(item) if item.name == "pdx_status_t"));
}

#[test]
fn parses_real_pdcm_program_subset_fixture() {
    let schema = read_schema("xdr/real/pdcm_program_basic.x");

    assert!(matches!(&schema.items[0], Item::Struct(item) if item.name == "pdcm_copy_arg_t"));
    assert!(matches!(&schema.items[1], Item::Program(item) if item.name == "PDCM_PROGRAM"));

    let Item::Program(program) = &schema.items[1] else {
        panic!("expected program item");
    };

    assert_eq!(program.number, 100666);
    assert_eq!(program.versions.len(), 1);
    assert_eq!(program.versions[0].name, "PDCM_RPC_V8");
    assert_eq!(program.versions[0].procedures.len(), 2);
    assert_eq!(program.versions[0].procedures[0].name, "PDCM_NULL");
    assert_eq!(
        program.versions[0].procedures[0].argument_type,
        TypeSpec::Void
    );
}

#[test]
fn parser_snapshot_matches_expected_output_for_real_fixtures() {
    assert_snapshot(
        "xdr/real/pd_types_basic.x",
        "expected/pd_types_basic.ast.txt",
    );
    assert_snapshot(
        "xdr/real/pdcm_program_basic.x",
        "expected/pdcm_program_basic.ast.txt",
    );
}

#[test]
fn parser_rejects_unsupported_union_constructs() {
    let error = parse_x_file(fixture("xdr/synthetic/unsupported_union.x"))
        .expect_err("union support should fail closed");

    assert_eq!(
        error,
        GeneratorError::UnsupportedConstruct(
            "union declarations are not supported yet".to_string()
        )
    );
}

#[test]
fn parser_ignores_percent_include_lines() {
    let source = "%#include \"pd/pd_types.h\"\nconst VALUE = 1;\n";
    let schema = parse_x_source(source).expect("include line should be ignored");

    assert_eq!(schema.items.len(), 1);
    assert!(matches!(&schema.items[0], Item::Const(item) if item.name == "VALUE"));
}

fn assert_snapshot(fixture_path: &str, expected_path: &str) {
    let actual = read_schema(fixture_path).render_snapshot();
    let expected =
        fs::read_to_string(fixture(expected_path)).expect("expected snapshot should exist");

    assert_eq!(actual, expected);
}
