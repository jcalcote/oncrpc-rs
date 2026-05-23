//! Core generator scaffolding for `.x`-based XDR type generation and ONC RPC
//! client/server stub generation.

use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GeneratorError {
    #[error("xdr/rpc generation is not implemented yet for {0}")]
    NotImplemented(String),
}

pub fn generate_from_x_file(path: impl AsRef<Path>) -> Result<(), GeneratorError> {
    Err(GeneratorError::NotImplemented(
        path.as_ref().display().to_string(),
    ))
}

pub fn fixture_root() -> &'static str {
    "../../tests/fixtures"
}
