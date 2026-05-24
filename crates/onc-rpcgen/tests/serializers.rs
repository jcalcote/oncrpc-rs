use bytes::Bytes;
use onc_rpc_xdr::{XdrDecode, XdrEncode};

#[allow(non_camel_case_types, non_snake_case, dead_code)]
mod storage_types_basic {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/expected/storage_types_basic.rs.txt"
    ));
}

#[allow(non_camel_case_types, non_snake_case, dead_code)]
mod rfc4506_parser_features {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/expected/rfc4506_parser_features.rs.txt"
    ));
}

#[allow(non_camel_case_types, non_snake_case, dead_code)]
mod common_types {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/expected/common_types.generated.types.rs.txt"
    ));
}

#[allow(non_camel_case_types, non_snake_case, dead_code)]
mod nfs_support {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/expected/nfs_support.generated.types.rs.txt"
    ));
}

#[allow(non_camel_case_types, non_snake_case, dead_code)]
mod transfer_types {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/expected/transfer_types.generated.types.rs.txt"
    ));
}

#[allow(non_camel_case_types, non_snake_case, dead_code)]
mod blob_service_basic {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/expected/blob_service_basic.generated.types.rs.txt"
    ));
}

#[test]
fn generated_struct_and_enum_round_trip() {
    let timestamp = storage_types_basic::timestamp_t {
        seconds: 7,
        nseconds: 9,
    };
    let encoded = timestamp.to_xdr_bytes().expect("encode");
    assert_eq!(&encoded[..], &[0, 0, 0, 7, 0, 0, 0, 9]);
    let decoded = storage_types_basic::timestamp_t::from_xdr_bytes(&encoded).expect("decode");
    assert_eq!(decoded, timestamp);

    let status = storage_types_basic::status_t::STATUS_STALE;
    let encoded = status.to_xdr_bytes().expect("encode");
    let decoded = storage_types_basic::status_t::from_xdr_bytes(&encoded).expect("decode");
    assert_eq!(decoded, status);

    let error = storage_types_basic::status_t::from_xdr_bytes(&[0, 0, 0, 3])
        .expect_err("unknown enum discriminant should fail");
    assert_eq!(error, onc_rpc_xdr::XdrError::InvalidEnum(3));
}

#[test]
fn generated_union_and_optional_round_trip() {
    let record = rfc4506_parser_features::full_record_t {
        fixed: [1, 2, 3, 4, 5, 6, 7, 8],
        next: Some(Box::new(1.25)),
    };
    let encoded = record.to_xdr_bytes().expect("encode");
    let decoded = rfc4506_parser_features::full_record_t::from_xdr_bytes(&encoded).expect("decode");
    assert_eq!(decoded, record);

    let payload = rfc4506_parser_features::payload_t::Case2 {
        text: "blob".to_string(),
    };
    let encoded = payload.to_xdr_bytes().expect("encode");
    assert_eq!(
        &encoded[..],
        &[0, 0, 0, 2, 0, 0, 0, 4, b'b', b'l', b'o', b'b']
    );
    let decoded = rfc4506_parser_features::payload_t::from_xdr_bytes(&encoded).expect("decode");
    assert_eq!(decoded, payload);

    let default_payload = rfc4506_parser_features::payload_t::Default { discriminant: 99 };
    let encoded = default_payload.to_xdr_bytes().expect("encode");
    let decoded = rfc4506_parser_features::payload_t::from_xdr_bytes(&encoded).expect("decode");
    assert_eq!(decoded, default_payload);
}

#[test]
fn generated_cross_module_types_round_trip() {
    let request = blob_service_basic::copy_request_t {
        job_id: 77,
        job_update: 9,
        source_instance: transfer_types::instance_info_t {
            source_handle: Bytes::from_static(b"remote-handle"),
        },
    };

    let encoded = request.to_xdr_bytes().expect("encode");
    let decoded = blob_service_basic::copy_request_t::from_xdr_bytes(&encoded).expect("decode");
    assert_eq!(decoded, request);
}
