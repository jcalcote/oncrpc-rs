//! Generator scaffolding for `.x`-based ONC RPC code generation.

use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GeneratorError {
    #[error("rpc generation is not implemented yet for {0}")]
    NotImplemented(String),
}

pub fn generate_from_x_file(path: impl AsRef<Path>) -> Result<(), GeneratorError> {
    Err(GeneratorError::NotImplemented(
        path.as_ref().display().to_string(),
    ))
}

