use onc_rpcgen::{
    DeclaratorModifier, Item, Schema, TypeSpec, UnionCaseLabel, ValueExpr, fixture_root,
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

    let Item::Struct(struct_decl) = &schema.items[4] else {
        panic!("expected struct item");
    };
    assert_eq!(struct_decl.body.declarations.len(), 2);
    assert_eq!(struct_decl.body.declarations[0].declarator.name, "seconds");
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
    assert_snapshot(
        "xdr/synthetic/rfc4506_parser_features.x",
        "expected/rfc4506_parser_features.ast.txt",
    );
}

#[test]
fn parser_supports_remaining_rfc4506_constructs() {
    let schema = read_schema("xdr/synthetic/rfc4506_parser_features.x");

    assert!(matches!(&schema.items[0], Item::Const(item) if item.name == "OCTAL_BOUND"));
    assert!(matches!(&schema.items[1], Item::Typedef(item) if item.target == TypeSpec::Float));
    assert!(matches!(&schema.items[2], Item::Typedef(item) if item.target == TypeSpec::Double));
    assert!(matches!(
        &schema.items[3],
        Item::Typedef(item) if item.target == TypeSpec::Quadruple
    ));

    let Item::Struct(struct_decl) = &schema.items[4] else {
        panic!("expected struct item");
    };
    assert_eq!(
        struct_decl.body.declarations[0].declarator.modifier,
        Some(DeclaratorModifier::FixedArray(ValueExpr::Identifier(
            "OCTAL_BOUND".to_string()
        )))
    );
    assert_eq!(
        struct_decl.body.declarations[1].declarator.modifier,
        Some(DeclaratorModifier::Optional)
    );

    let Item::Union(union_decl) = &schema.items[5] else {
        panic!("expected union item");
    };
    assert_eq!(union_decl.body.arms.len(), 3);
    assert!(matches!(
        union_decl.body.arms[1].labels.as_slice(),
        [
            UnionCaseLabel::Case(ValueExpr::Number(2)),
            UnionCaseLabel::Case(ValueExpr::Number(3))
        ]
    ));
    assert!(matches!(
        union_decl.body.arms[2].labels.as_slice(),
        [UnionCaseLabel::Default]
    ));

    let Item::Typedef(typedef_decl) = &schema.items[6] else {
        panic!("expected typedef item");
    };
    assert!(matches!(typedef_decl.target, TypeSpec::Enum(_)));
}

#[test]
fn parser_supports_inline_type_specifiers_in_source() {
    let source = "\
typedef struct { unsigned int x; unsigned int y; } point_t;\n\
typedef union switch (int kind) { case 1: int value; default: void nothing; } payload_t;\n";
    let schema = parse_x_source(source).expect("inline type specifiers should parse");

    assert_eq!(schema.items.len(), 2);
    assert!(
        matches!(&schema.items[0], Item::Typedef(item) if matches!(item.target, TypeSpec::Struct(_)))
    );
    assert!(
        matches!(&schema.items[1], Item::Typedef(item) if matches!(item.target, TypeSpec::Union(_)))
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
