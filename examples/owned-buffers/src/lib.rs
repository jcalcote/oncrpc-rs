//! Shared protocol generated from `buffer_echo.x`.
#[allow(
    non_camel_case_types,
    non_snake_case,
    dead_code,
    clippy::module_inception
)]
pub mod buffer_echo {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/generated/buffer_echo.types.rs"
    ));
}

#[allow(
    non_camel_case_types,
    non_snake_case,
    dead_code,
    clippy::module_inception
)]
pub mod buffer_echo_stubs {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/generated/buffer_echo.stubs.rs"
    ));
}
