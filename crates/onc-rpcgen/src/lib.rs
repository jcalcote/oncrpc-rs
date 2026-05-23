//! Core generator scaffolding for `.x`-based XDR type generation and ONC RPC
//! client/server stub generation.

mod ast;
mod emit;
mod parser;

pub use ast::*;
pub use emit::{emit_rust_stubs, emit_rust_types};

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum GeneratorError {
    #[error("failed to read XDR source from {path}: {message}")]
    Io { path: String, message: String },
    #[error("failed to resolve include {include} from {including}")]
    IncludeResolution { including: String, include: String },
    #[error("unsupported XDR/RPC construct: {0}")]
    UnsupportedConstruct(String),
    #[error("failed to parse XDR/RPC source: {0}")]
    Parse(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadOptions {
    pub include_dirs: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerateOptions {
    pub module_name: Option<String>,
    pub emit_types: bool,
    pub emit_stubs: bool,
}

impl Default for GenerateOptions {
    fn default() -> Self {
        Self {
            module_name: None,
            emit_types: true,
            emit_stubs: true,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GeneratedOutputs {
    pub module_name: String,
    pub types: Option<String>,
    pub stubs: Option<String>,
}

pub fn parse_x_file(path: impl AsRef<Path>) -> Result<Schema, GeneratorError> {
    parse_x_file_with_options(path, &LoadOptions::default())
}

pub fn parse_x_file_with_options(
    path: impl AsRef<Path>,
    options: &LoadOptions,
) -> Result<Schema, GeneratorError> {
    let path = path.as_ref();
    let mut visited = HashSet::new();
    load_schema_recursive(path, options, &mut visited)
}

pub fn parse_x_source(source: &str) -> Result<Schema, GeneratorError> {
    parser::parse_x_source(source)
}

pub fn emit_rust_types_from_x_file(path: impl AsRef<Path>) -> Result<String, GeneratorError> {
    emit_rust_types_from_x_file_with_options(path, &LoadOptions::default())
}

pub fn emit_rust_types_from_x_file_with_options(
    path: impl AsRef<Path>,
    load: &LoadOptions,
) -> Result<String, GeneratorError> {
    let schema = parse_x_file_with_options(path, load)?;
    emit_rust_types(&schema)
}

pub fn emit_rust_types_from_x_source(source: &str) -> Result<String, GeneratorError> {
    let schema = parse_x_source(source)?;
    emit_rust_types(&schema)
}

pub fn emit_rust_stubs_from_x_file(path: impl AsRef<Path>) -> Result<String, GeneratorError> {
    emit_rust_stubs_from_x_file_with_options(path, &LoadOptions::default())
}

pub fn emit_rust_stubs_from_x_file_with_options(
    path: impl AsRef<Path>,
    load: &LoadOptions,
) -> Result<String, GeneratorError> {
    let schema = parse_x_file_with_options(path, load)?;
    emit_rust_stubs(&schema)
}

pub fn emit_rust_stubs_from_x_source(source: &str) -> Result<String, GeneratorError> {
    let schema = parse_x_source(source)?;
    emit_rust_stubs(&schema)
}

pub fn generate_from_x_file(path: impl AsRef<Path>) -> Result<GeneratedOutputs, GeneratorError> {
    generate_from_x_file_with_options(path, &LoadOptions::default(), &GenerateOptions::default())
}

pub fn generate_from_x_file_with_options(
    path: impl AsRef<Path>,
    load: &LoadOptions,
    options: &GenerateOptions,
) -> Result<GeneratedOutputs, GeneratorError> {
    let path = path.as_ref();
    let schema = parse_x_file_with_options(path, load)?;
    generate_rust(
        &schema,
        module_name_for_path(path, options.module_name.as_deref()),
        options,
    )
}

pub fn generate_rust(
    schema: &Schema,
    module_name: String,
    options: &GenerateOptions,
) -> Result<GeneratedOutputs, GeneratorError> {
    Ok(GeneratedOutputs {
        module_name,
        types: if options.emit_types {
            Some(emit_rust_types(schema)?)
        } else {
            None
        },
        stubs: if options.emit_stubs {
            Some(emit_rust_stubs(schema)?)
        } else {
            None
        },
    })
}

pub fn module_name_for_path(path: &Path, override_name: Option<&str>) -> String {
    if let Some(name) = override_name {
        return sanitize_module_name(name);
    }

    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("generated");
    sanitize_module_name(stem)
}

pub fn fixture_root() -> &'static str {
    "../../tests/fixtures"
}

fn load_schema_recursive(
    path: &Path,
    options: &LoadOptions,
    visited: &mut HashSet<PathBuf>,
) -> Result<Schema, GeneratorError> {
    let canonical = path.canonicalize().map_err(|error| GeneratorError::Io {
        path: path.display().to_string(),
        message: error.to_string(),
    })?;

    if !visited.insert(canonical.clone()) {
        return Ok(Schema { items: Vec::new() });
    }

    let source = fs::read_to_string(&canonical).map_err(|error| GeneratorError::Io {
        path: canonical.display().to_string(),
        message: error.to_string(),
    })?;

    let mut merged = Schema { items: Vec::new() };
    for include in extract_includes(&source) {
        let include_path = resolve_include(&canonical, &include, options).ok_or_else(|| {
            GeneratorError::IncludeResolution {
                including: canonical.display().to_string(),
                include: include.clone(),
            }
        })?;
        let child = load_schema_recursive(&include_path, options, visited)?;
        merged.items.extend(child.items);
    }

    let current = parse_x_source(&source)?;
    merged.items.extend(current.items);
    Ok(merged)
}

fn extract_includes(source: &str) -> Vec<String> {
    source
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if !line.starts_with("%#include") {
                return None;
            }
            let first = line.find('"')?;
            let rest = &line[first + 1..];
            let second = rest.find('"')?;
            Some(rest[..second].to_string())
        })
        .collect()
}

fn resolve_include(including: &Path, include: &str, options: &LoadOptions) -> Option<PathBuf> {
    let include_path = Path::new(include);
    let include_x = include_path.with_extension("x");
    let basename_x = include_path.file_stem().map(|stem| {
        let mut p = PathBuf::from(stem);
        p.set_extension("x");
        p
    });

    let mut roots = Vec::new();
    if let Some(parent) = including.parent() {
        roots.push(parent.to_path_buf());
    }
    roots.extend(options.include_dirs.iter().cloned());

    for root in roots {
        let direct = root.join(&include_x);
        if direct.is_file() {
            return Some(direct);
        }
        if let Some(base) = &basename_x {
            let fallback = root.join(base);
            if fallback.is_file() {
                return Some(fallback);
            }
        }
    }

    None
}

fn sanitize_module_name(name: &str) -> String {
    let mut out = String::new();

    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push('_');
        }
    }

    while out.contains("__") {
        out = out.replace("__", "_");
    }

    out.trim_matches('_').to_string()
}
