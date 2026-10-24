use onc_rpcgen::{emit_rust_stubs, emit_rust_types, parse_x_source};

#[allow(non_camel_case_types, non_snake_case, dead_code)]
mod owned_buffers {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/expected/owned_buffers.generated.types.rs.txt"
    ));
}

#[test]
fn generated_optional_alias_projection_encodes_and_decodes_typed_metadata() {
    use bytes::Bytes;
    use onc_rpc_xdr::{XdrDecode, XdrEncode, XdrEncodeSegments, XdrReplyMetadata};
    use owned_buffers::*;
    let body = Bytes::from_owner(vec![1, 2, 3]);
    let source = response::True {
        value: wrapper {
            body: Some(Box::new(record {
                ratio: 1.25,
                value: body.clone(),
            })),
        },
    };
    let expected = source.to_xdr_bytes().unwrap();
    let projected: responseSegments = source.into();
    let segments = projected.to_xdr_segments().unwrap();
    assert!(
        segments
            .iter()
            .any(|segment| segment.as_ptr() == body.as_ptr())
    );
    let actual: Vec<u8> = segments
        .iter()
        .flat_map(|segment| segment.iter().copied())
        .collect();
    assert_eq!(actual, expected);
    let mut input = &actual[..];
    let metadata = responseMetadata::decode_xdr(&mut input).unwrap();
    assert_eq!(metadata.opaque_len(), Some(3));
    assert_eq!(input, &[1, 2, 3, 0]);
    assert_eq!(responseMetadata::MAX_PREFIX_BYTES, 16);
    let none = response::True {
        value: wrapper { body: None },
    }
    .to_xdr_bytes()
    .unwrap();
    let metadata = responseMetadata::from_xdr_bytes(&none).unwrap();
    assert_eq!(metadata.opaque_len(), None);
}

fn generate(source: &str) -> (String, String) {
    let schema = parse_x_source(source).unwrap();
    (
        emit_rust_types(&schema).unwrap(),
        emit_rust_stubs(&schema).unwrap(),
    )
}

#[test]
fn terminal_body_gets_distinct_metadata_and_segmented_representations() {
    let (types, stubs) = generate(
        r#"
const LIMIT = 64;
struct attributes { unsigned hyper size; };
union optional_attributes switch (bool present) {
case TRUE: attributes value;
case FALSE: void;
};
struct result_ok { optional_attributes attrs; unsigned int count; bool eof; opaque data<LIMIT>; };
union result switch (int status) {
case 0: result_ok value;
default: optional_attributes attrs;
};
program TEST { version V1 { result FETCH(void) = 1; } = 1; } = 300002;
"#,
    );
    assert!(types.contains("pub struct result_okMetadata"));
    assert!(types.contains("pub enum resultMetadata"));
    assert!(types.contains("OpaqueLength<{ LIMIT as usize }>"));
    assert!(types.contains("const MAX_PREFIX_BYTES: usize = 28;"));
    assert!(types.contains("pub struct result_okSegments"));
    assert!(types.contains("OpaqueSegments<{ LIMIT as usize }>"));
    assert!(!types.contains("impl XdrEncode for resultMetadata"));
    assert!(stubs.contains("pub async fn fetch_into<"));
    assert!(stubs.contains("pub fn try_fetch_into<"));
    assert!(stubs.contains("pub async fn fetch_segmented<"));
}

#[test]
fn variable_control_fields_and_nonterminal_bodies_do_not_claim_receive_support() {
    for fields in [
        "string label<>; opaque data<>;",
        "unsigned int values<>; opaque data<>;",
        "opaque first<>; opaque data<>;",
        "opaque data<>; unsigned int checksum;",
    ] {
        let (types, stubs) = generate(&format!(
            "struct result {{ {fields} }}; program TEST {{ version V1 {{ result FETCH(void) = 1; }} = 1; }} = 300002;"
        ));
        assert!(!types.contains("resultMetadata"), "{fields}");
        assert!(!stubs.contains("fetch_into"), "{fields}");
        assert!(stubs.contains("fetch_segmented"), "{fields}");
        assert!(types.contains("impl onc_rpc_xdr::XdrEncodeSegments for result"));
    }
}

#[test]
fn recursive_control_layout_does_not_claim_a_static_prefix_bound() {
    let (types, stubs) = generate(
        r#"
struct node { unsigned int value; node *next; };
struct result { node nodes; opaque data<>; };
program TEST { version V1 { result FETCH(void) = 1; } = 1; } = 300002;
"#,
    );
    assert!(!types.contains("resultMetadata"));
    assert!(!stubs.contains("fetch_into"));
}

#[test]
fn projection_names_cannot_silently_shadow_user_types() {
    for suffix in ["Metadata", "Segments"] {
        let source = format!(
            "struct body {{ opaque data<>; }}; struct body{suffix} {{ unsigned int value; }};"
        );
        let error = emit_rust_types(&parse_x_source(&source).unwrap()).unwrap_err();
        assert!(error.to_string().contains("collides"));
    }
}

#[test]
fn generated_owned_method_names_cannot_collide_with_idl_procedures() {
    for conflicting in [
        "FETCH_INTO",
        "FETCH_SEGMENTED",
        "TRY_FETCH_INTO",
        "TRY_FETCH_SEGMENTED",
    ] {
        let source = format!(
            "struct result {{ opaque data<>; }}; program TEST {{ version V1 {{ result FETCH(void) = 1; void {conflicting}(void) = 2; }} = 1; }} = 300002;"
        );
        let schema = parse_x_source(&source).unwrap();
        let error = emit_rust_stubs(&schema).unwrap_err();
        assert!(
            error.to_string().contains("method name collides"),
            "{conflicting}: {error}"
        );
    }
}

#[test]
fn segmented_typedef_chain_retains_declared_bound_without_treating_segments_as_bytes() {
    let (types, _) = generate(
        r#"
typedef opaque blob<64>;
typedef blob alias;
struct request { alias data; };
"#,
    );
    assert!(types.contains("pub type aliasSegments = blobSegments;"));
    let generated = types
        .split("impl onc_rpc_xdr::XdrEncodeSegments for requestSegments")
        .nth(1)
        .unwrap();
    assert!(!generated.contains("output.opaque(&self.data"));
}
