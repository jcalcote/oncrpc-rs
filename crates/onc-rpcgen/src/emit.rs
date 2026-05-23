use crate::GeneratorError;
use crate::ast::*;
use std::fmt::Write;

pub fn emit_rust_types(schema: &Schema) -> Result<String, GeneratorError> {
    let uses_bytes = schema_uses_bytes(schema);
    let mut out = String::new();

    if uses_bytes {
        out.push_str("use bytes::Bytes;\n\n");
    }

    for item in &schema.items {
        emit_item(item, 0, &mut out)?;
    }

    Ok(out)
}

pub fn emit_rust_stubs(schema: &Schema) -> Result<String, GeneratorError> {
    let mut out = String::new();

    if !schema
        .items
        .iter()
        .any(|item| matches!(item, Item::Program(_)))
    {
        return Ok(out);
    }

    out.push_str("use bytes::Bytes;\n\n");

    for item in &schema.items {
        if let Item::Program(item) = item {
            emit_program_stubs(item, 0, &mut out)?;
        }
    }

    Ok(out)
}

fn emit_item(item: &Item, indent: usize, out: &mut String) -> Result<(), GeneratorError> {
    match item {
        Item::Const(item) => emit_const(item, indent, out),
        Item::Typedef(item) => emit_typedef(item, indent, out),
        Item::Struct(item) => emit_struct(item, indent, out),
        Item::Enum(item) => emit_enum(item, indent, out),
        Item::Program(item) => emit_program(item, indent, out),
    }
}

fn emit_const(item: &ConstDecl, indent: usize, out: &mut String) -> Result<(), GeneratorError> {
    line(
        indent,
        out,
        format_args!(
            "pub const {}: i64 = {};",
            item.name,
            render_value(&item.value)
        ),
    );
    out.push('\n');
    Ok(())
}

fn emit_typedef(item: &TypedefDecl, indent: usize, out: &mut String) -> Result<(), GeneratorError> {
    let rust_type = rust_type_for_declaration(&item.target, &item.declarator)?;
    line(
        indent,
        out,
        format_args!("pub type {} = {};", item.declarator.name, rust_type),
    );
    out.push('\n');
    Ok(())
}

fn emit_struct(item: &StructDecl, indent: usize, out: &mut String) -> Result<(), GeneratorError> {
    line(
        indent,
        out,
        format_args!("#[derive(Debug, Clone, PartialEq, Eq)]"),
    );
    line(indent, out, format_args!("pub struct {} {{", item.name));

    for field in &item.fields {
        let rust_type = rust_type_for_declaration(&field.field_type, &field.declarator)?;
        line(
            indent + 4,
            out,
            format_args!("pub {}: {},", field.declarator.name, rust_type),
        );
    }

    line(indent, out, format_args!("}}"));
    out.push('\n');
    Ok(())
}

fn emit_enum(item: &EnumDecl, indent: usize, out: &mut String) -> Result<(), GeneratorError> {
    line(
        indent,
        out,
        format_args!("#[derive(Debug, Clone, Copy, PartialEq, Eq)]"),
    );
    line(indent, out, format_args!("#[repr(i32)]"));
    line(indent, out, format_args!("pub enum {} {{", item.name));

    for variant in &item.variants {
        line(
            indent + 4,
            out,
            format_args!("{} = {},", variant.name, render_value(&variant.value)),
        );
    }

    line(indent, out, format_args!("}}"));
    out.push('\n');
    Ok(())
}

fn emit_program(item: &ProgramDecl, indent: usize, out: &mut String) -> Result<(), GeneratorError> {
    let module_name = to_snake_case(&item.name);
    line(indent, out, format_args!("pub mod {} {{", module_name));
    line(
        indent + 4,
        out,
        format_args!("pub const PROGRAM: u32 = {};", item.number),
    );
    out.push('\n');

    for version in &item.versions {
        let version_module = to_snake_case(&version.name);
        line(
            indent + 4,
            out,
            format_args!("pub mod {} {{", version_module),
        );
        line(
            indent + 8,
            out,
            format_args!("pub const VERSION: u32 = {};", version.number),
        );
        for procedure in &version.procedures {
            line(
                indent + 8,
                out,
                format_args!("pub const {}: u32 = {};", procedure.name, procedure.number),
            );
        }
        line(indent + 4, out, format_args!("}}"));
        out.push('\n');
    }

    line(indent, out, format_args!("}}"));
    out.push('\n');
    Ok(())
}

fn emit_program_stubs(
    item: &ProgramDecl,
    indent: usize,
    out: &mut String,
) -> Result<(), GeneratorError> {
    let module_name = to_snake_case(&item.name);
    line(indent, out, format_args!("pub mod {} {{", module_name));
    line(
        indent + 4,
        out,
        format_args!("pub const PROGRAM: u32 = {};", item.number),
    );
    out.push('\n');

    for version in &item.versions {
        emit_version_stubs(version, indent + 4, out)?;
    }

    line(indent, out, format_args!("}}"));
    out.push('\n');
    Ok(())
}

fn emit_version_stubs(
    version: &VersionDecl,
    indent: usize,
    out: &mut String,
) -> Result<(), GeneratorError> {
    let module_name = to_snake_case(&version.name);
    let client_name = format!("{}Client", version.name);
    let service_name = format!("{}Service", version.name);
    let dispatch_name = format!("{}Dispatch", version.name);

    line(indent, out, format_args!("pub mod {} {{", module_name));
    line(
        indent + 4,
        out,
        format_args!("pub const VERSION: u32 = {};", version.number),
    );
    for procedure in &version.procedures {
        line(
            indent + 4,
            out,
            format_args!("pub const {}: u32 = {};", procedure.name, procedure.number),
        );
    }
    out.push('\n');

    line(
        indent + 4,
        out,
        format_args!("pub struct {}<T> {{", client_name),
    );
    line(
        indent + 8,
        out,
        format_args!("client: onc_rpc_runtime::Client<T>,"),
    );
    line(indent + 4, out, format_args!("}}"));
    out.push('\n');

    line(
        indent + 4,
        out,
        format_args!(
            "impl<T> {}<T> where T: onc_rpc_runtime::ClientTransport {{",
            client_name
        ),
    );
    line(
        indent + 8,
        out,
        format_args!("pub fn new(client: onc_rpc_runtime::Client<T>) -> Self {{"),
    );
    line(indent + 12, out, format_args!("Self {{ client }}"));
    line(indent + 8, out, format_args!("}}"));
    out.push('\n');

    for procedure in &version.procedures {
        emit_client_method(procedure, indent + 8, out)?;
    }

    line(indent + 4, out, format_args!("}}"));
    out.push('\n');

    line(
        indent + 4,
        out,
        format_args!("pub trait {} {{", service_name),
    );
    for procedure in &version.procedures {
        emit_service_method(procedure, indent + 8, out);
    }
    line(indent + 4, out, format_args!("}}"));
    out.push('\n');

    line(
        indent + 4,
        out,
        format_args!("pub struct {}<T> {{", dispatch_name),
    );
    line(indent + 8, out, format_args!("inner: T,"));
    line(indent + 4, out, format_args!("}}"));
    out.push('\n');

    line(
        indent + 4,
        out,
        format_args!("impl<T> {}<T> {{", dispatch_name),
    );
    line(
        indent + 8,
        out,
        format_args!("pub fn new(inner: T) -> Self {{"),
    );
    line(indent + 12, out, format_args!("Self {{ inner }}"));
    line(indent + 8, out, format_args!("}}"));
    line(indent + 4, out, format_args!("}}"));
    out.push('\n');

    line(
        indent + 4,
        out,
        format_args!(
            "impl<T> onc_rpc_server::Dispatch for {}<T> where T: {} + Send + Sync + 'static {{",
            dispatch_name, service_name
        ),
    );
    line(
        indent + 8,
        out,
        format_args!(
            "fn dispatch(&self, request: onc_rpc_server::RequestContext) -> Result<onc_rpc_server::ResponsePayload, onc_rpc_server::DispatchError> {{"
        ),
    );
    line(
        indent + 12,
        out,
        format_args!("match request.procedure.0 {{"),
    );
    for procedure in &version.procedures {
        emit_dispatch_arm(procedure, indent + 16, out)?;
    }
    line(
        indent + 16,
        out,
        format_args!("_ => Err(onc_rpc_server::DispatchError::ProcedureUnavailable),"),
    );
    line(indent + 12, out, format_args!("}}"));
    line(indent + 8, out, format_args!("}}"));
    line(indent + 4, out, format_args!("}}"));
    out.push('\n');

    line(indent, out, format_args!("}}"));
    out.push('\n');

    Ok(())
}

fn emit_client_method(
    procedure: &ProcedureDecl,
    indent: usize,
    out: &mut String,
) -> Result<(), GeneratorError> {
    let method_name = to_snake_case(&procedure.name);
    let signature = if matches!(procedure.argument_type, TypeSpec::Void) {
        format!(
            "pub fn {}(&self) -> Result<onc_rpc_runtime::CallResponse, onc_rpc_runtime::RuntimeError> {{",
            method_name
        )
    } else {
        format!(
            "pub fn {}(&self, payload: Bytes) -> Result<onc_rpc_runtime::CallResponse, onc_rpc_runtime::RuntimeError> {{",
            method_name
        )
    };
    line(indent, out, format_args!("{signature}"));
    line(
        indent + 4,
        out,
        format_args!("let request = onc_rpc_runtime::CallRequest::new("),
    );
    line(
        indent + 8,
        out,
        format_args!(
            "onc_rpc_runtime::ProgramVersion {{ program: super::PROGRAM, version: VERSION }},"
        ),
    );
    line(
        indent + 8,
        out,
        format_args!("onc_rpc_runtime::Procedure({}),", procedure.name),
    );
    if matches!(procedure.argument_type, TypeSpec::Void) {
        line(indent + 8, out, format_args!("Bytes::new(),"));
    } else {
        line(indent + 8, out, format_args!("payload,"));
    }
    line(indent + 4, out, format_args!(");"));
    line(indent + 4, out, format_args!("self.client.call(request)"));
    line(indent, out, format_args!("}}"));
    out.push('\n');
    Ok(())
}

fn emit_service_method(procedure: &ProcedureDecl, indent: usize, out: &mut String) {
    let method_name = to_snake_case(&procedure.name);
    if matches!(procedure.argument_type, TypeSpec::Void) {
        line(
            indent,
            out,
            format_args!(
                "fn {}(&self) -> Result<Bytes, onc_rpc_server::DispatchError>;",
                method_name
            ),
        );
    } else {
        line(
            indent,
            out,
            format_args!(
                "fn {}(&self, payload: Bytes) -> Result<Bytes, onc_rpc_server::DispatchError>;",
                method_name
            ),
        );
    }
}

fn emit_dispatch_arm(
    procedure: &ProcedureDecl,
    indent: usize,
    out: &mut String,
) -> Result<(), GeneratorError> {
    let method_name = to_snake_case(&procedure.name);
    line(indent, out, format_args!("{} => {{", procedure.number));
    let invocation = if matches!(procedure.argument_type, TypeSpec::Void) {
        format!("self.inner.{}()", method_name)
    } else {
        format!("self.inner.{}(request.payload)", method_name)
    };
    line(
        indent + 4,
        out,
        format_args!("{invocation}.map(onc_rpc_server::ResponsePayload::success)"),
    );
    line(indent, out, format_args!("}}"));
    Ok(())
}

fn rust_type_for_declaration(
    target: &TypeSpec,
    declarator: &Declarator,
) -> Result<String, GeneratorError> {
    match &declarator.modifier {
        Some(DeclaratorModifier::VariableArray(_)) => match target {
            TypeSpec::Opaque => Ok("Bytes".to_string()),
            TypeSpec::String => Ok("String".to_string()),
            other => Ok(format!("Vec<{}>", rust_type_for_base(other))),
        },
        None => Ok(rust_type_for_base(target)),
    }
}

fn rust_type_for_base(target: &TypeSpec) -> String {
    match target {
        TypeSpec::Void => "()".to_string(),
        TypeSpec::Bool => "bool".to_string(),
        TypeSpec::Int => "i32".to_string(),
        TypeSpec::UnsignedInt => "u32".to_string(),
        TypeSpec::Hyper => "i64".to_string(),
        TypeSpec::UnsignedHyper => "u64".to_string(),
        TypeSpec::Opaque => "Bytes".to_string(),
        TypeSpec::String => "String".to_string(),
        TypeSpec::Identifier(name) => name.clone(),
    }
}

fn schema_uses_bytes(schema: &Schema) -> bool {
    schema.items.iter().any(item_uses_bytes)
}

fn item_uses_bytes(item: &Item) -> bool {
    match item {
        Item::Const(_) | Item::Enum(_) | Item::Program(_) => false,
        Item::Typedef(item) => declaration_uses_bytes(&item.target, &item.declarator),
        Item::Struct(item) => item
            .fields
            .iter()
            .any(|field| declaration_uses_bytes(&field.field_type, &field.declarator)),
    }
}

fn declaration_uses_bytes(target: &TypeSpec, declarator: &Declarator) -> bool {
    matches!(target, TypeSpec::Opaque)
        || matches!(
            (&declarator.modifier, target),
            (Some(DeclaratorModifier::VariableArray(_)), TypeSpec::Opaque)
        )
}

fn render_value(value: &ValueExpr) -> String {
    match value {
        ValueExpr::Number(value) => value.to_string(),
        ValueExpr::Identifier(name) => name.clone(),
    }
}

fn to_snake_case(input: &str) -> String {
    input.to_ascii_lowercase()
}

fn line(indent: usize, out: &mut String, args: std::fmt::Arguments<'_>) {
    let _ = write!(out, "{:indent$}", "", indent = indent);
    let _ = out.write_fmt(args);
    out.push('\n');
}
