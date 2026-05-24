#[allow(
    non_camel_case_types,
    non_snake_case,
    dead_code,
    clippy::module_inception
)]
pub mod time_service {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/generated/time_service.types.rs"
    ));
}

#[allow(
    non_camel_case_types,
    non_snake_case,
    dead_code,
    clippy::module_inception
)]
pub mod time_service_stubs {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/generated/time_service.stubs.rs"
    ));
}
