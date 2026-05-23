//! Core generator scaffolding for `.x`-based XDR type generation and ONC RPC
//! client/server stub generation.

mod ast;
mod emit;
mod parser;

pub use ast::*;
pub use emit::emit_rust_types;
use std::fs;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum GeneratorError {
    #[error("failed to read XDR source from {path}: {message}")]
    Io { path: String, message: String },
    #[error("unsupported XDR/RPC construct: {0}")]
    UnsupportedConstruct(String),
    #[error("failed to parse XDR/RPC source: {0}")]
    Parse(String),
    #[error("code generation is not implemented yet for {0}")]
    NotImplemented(String),
}

pub fn parse_x_file(path: impl AsRef<Path>) -> Result<Schema, GeneratorError> {
    let path = path.as_ref();
    let source = fs::read_to_string(path).map_err(|error| GeneratorError::Io {
        path: path.display().to_string(),
        message: error.to_string(),
    })?;
    parse_x_source(&source)
}

pub fn parse_x_source(source: &str) -> Result<Schema, GeneratorError> {
    parser::parse_x_source(source)
}

pub fn emit_rust_types_from_x_file(path: impl AsRef<Path>) -> Result<String, GeneratorError> {
    let schema = parse_x_file(path)?;
    emit_rust_types(&schema)
}

pub fn emit_rust_types_from_x_source(source: &str) -> Result<String, GeneratorError> {
    let schema = parse_x_source(source)?;
    emit_rust_types(&schema)
}

pub fn generate_from_x_file(path: impl AsRef<Path>) -> Result<(), GeneratorError> {
    let path = path.as_ref();
    parse_x_file(path)?;
    Err(GeneratorError::NotImplemented(path.display().to_string()))
}

pub fn fixture_root() -> &'static str {
    "../../tests/fixtures"
}
