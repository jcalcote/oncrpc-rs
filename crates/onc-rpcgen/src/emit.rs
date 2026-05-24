use crate::GeneratorError;
use crate::LoadedModule;
use crate::LoadedSchemaSet;
use crate::ast::*;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashSet;
use std::fmt::Write;

pub fn emit_rust_types(schema: &Schema) -> Result<String, GeneratorError> {
    let mut emitter = TypeEmitter::new(schema);
    emitter.emit_schema(schema)?;
    Ok(emitter.out)
}

pub fn emit_rust_types_for_module(
    module: &LoadedModule,
    loaded: &LoadedSchemaSet,
) -> Result<String, GeneratorError> {
    let mut emitter = TypeEmitter::new_for_module(module, loaded);
    emitter.emit_schema(&module.schema)?;
    Ok(emitter.out)
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

pub fn emit_rust_stubs_for_module(module: &LoadedModule) -> Result<String, GeneratorError> {
    emit_rust_stubs(&module.schema)
}

struct TypeEmitter {
    out: String,
    emitted_helpers: BTreeSet<String>,
    named_types: BTreeMap<String, NamedType>,
    current_module: Option<String>,
    type_owners: BTreeMap<String, String>,
    const_owners: BTreeMap<String, String>,
}

#[derive(Clone)]
enum NamedType {
    Typedef {
        target: TypeSpec,
        modifier: Option<DeclaratorModifier>,
    },
    Struct(StructBody),
    Enum,
    Union(UnionBody),
}

impl TypeEmitter {
    fn new(schema: &Schema) -> Self {
        let mut out = String::new();
        if schema_uses_bytes(schema) {
            out.push_str("use bytes::Bytes;\n\n");
        }
        Self {
            out,
            emitted_helpers: BTreeSet::new(),
            named_types: build_named_types(schema),
            current_module: None,
            type_owners: BTreeMap::new(),
            const_owners: BTreeMap::new(),
        }
    }

    fn new_for_module(module: &LoadedModule, loaded: &LoadedSchemaSet) -> Self {
        let mut out = String::new();
        if schema_uses_bytes(&module.schema) {
            out.push_str("use bytes::Bytes;\n\n");
        }
        Self {
            out,
            emitted_helpers: BTreeSet::new(),
            named_types: build_named_types_for_modules(&loaded.modules),
            current_module: Some(module.module_name.clone()),
            type_owners: build_type_owners(&loaded.modules),
            const_owners: build_const_owners(&loaded.modules),
        }
    }

    fn emit_schema(&mut self, schema: &Schema) -> Result<(), GeneratorError> {
        for item in &schema.items {
            self.emit_item(item, 0)?;
        }
        Ok(())
    }

    fn emit_item(&mut self, item: &Item, indent: usize) -> Result<(), GeneratorError> {
        match item {
            Item::Const(item) => self.emit_const(item, indent),
            Item::Typedef(item) => self.emit_typedef(item, indent),
            Item::Struct(item) => self.emit_named_struct(&item.name, &item.body, indent),
            Item::Enum(item) => self.emit_named_enum(&item.name, &item.body, indent),
            Item::Union(item) => self.emit_named_union(&item.name, &item.body, indent),
            Item::Program(item) => self.emit_program(item, indent),
        }
    }

    fn emit_const(&mut self, item: &ConstDecl, indent: usize) -> Result<(), GeneratorError> {
        let value = self.render_value(&item.value);
        line(
            indent,
            &mut self.out,
            format_args!("pub const {}: i64 = {};", item.name, value),
        );
        self.out.push('\n');
        Ok(())
    }

    fn emit_typedef(&mut self, item: &TypedefDecl, indent: usize) -> Result<(), GeneratorError> {
        match &item.target {
            TypeSpec::Enum(body) if item.declarator.modifier.is_none() => {
                self.emit_named_enum(&item.declarator.name, body, indent)
            }
            TypeSpec::Struct(body) if item.declarator.modifier.is_none() => {
                self.emit_named_struct(&item.declarator.name, body, indent)
            }
            TypeSpec::Union(body) if item.declarator.modifier.is_none() => {
                self.emit_named_union(&item.declarator.name, body, indent)
            }
            TypeSpec::Enum(body) => {
                let helper_name = format!("{}_value", item.declarator.name);
                self.emit_named_enum(&helper_name, body, indent)?;
                let rust_type = self.rust_type_for_declaration(
                    &TypeSpec::Identifier(helper_name),
                    &item.declarator,
                    &item.declarator.name,
                    indent,
                )?;
                line(
                    indent,
                    &mut self.out,
                    format_args!("pub type {} = {};", item.declarator.name, rust_type),
                );
                self.out.push('\n');
                Ok(())
            }
            TypeSpec::Struct(body) => {
                let helper_name = format!("{}_value", item.declarator.name);
                self.emit_named_struct(&helper_name, body, indent)?;
                let rust_type = self.rust_type_for_declaration(
                    &TypeSpec::Identifier(helper_name),
                    &item.declarator,
                    &item.declarator.name,
                    indent,
                )?;
                line(
                    indent,
                    &mut self.out,
                    format_args!("pub type {} = {};", item.declarator.name, rust_type),
                );
                self.out.push('\n');
                Ok(())
            }
            TypeSpec::Union(body) => {
                let helper_name = format!("{}_value", item.declarator.name);
                self.emit_named_union(&helper_name, body, indent)?;
                let rust_type = self.rust_type_for_declaration(
                    &TypeSpec::Identifier(helper_name),
                    &item.declarator,
                    &item.declarator.name,
                    indent,
                )?;
                line(
                    indent,
                    &mut self.out,
                    format_args!("pub type {} = {};", item.declarator.name, rust_type),
                );
                self.out.push('\n');
                Ok(())
            }
            _ => {
                let rust_type = self.rust_type_for_declaration(
                    &item.target,
                    &item.declarator,
                    &item.declarator.name,
                    indent,
                )?;
                line(
                    indent,
                    &mut self.out,
                    format_args!("pub type {} = {};", item.declarator.name, rust_type),
                );
                self.out.push('\n');
                Ok(())
            }
        }
    }

    fn emit_named_struct(
        &mut self,
        name: &str,
        body: &StructBody,
        indent: usize,
    ) -> Result<(), GeneratorError> {
        if !self.emitted_helpers.insert(name.to_string()) {
            return Ok(());
        }

        for declaration in &body.declarations {
            let hint = format!("{}_{}", name, declaration.declarator.name);
            self.emit_inline_helper_types(&declaration.type_spec, &hint, indent)?;
        }

        emit_derive_block(
            indent,
            self.struct_is_eq(body),
            &mut self.out,
            DeriveKind::Struct,
        );
        line(
            indent,
            &mut self.out,
            format_args!("pub struct {} {{", name),
        );
        for declaration in &body.declarations {
            let hint = format!("{}_{}", name, declaration.declarator.name);
            let rust_type = self.rust_type_for_declaration(
                &declaration.type_spec,
                &declaration.declarator,
                &hint,
                indent,
            )?;
            line(
                indent + 4,
                &mut self.out,
                format_args!("pub {}: {},", declaration.declarator.name, rust_type),
            );
        }
        line(indent, &mut self.out, format_args!("}}"));
        self.out.push('\n');
        Ok(())
    }

    fn emit_named_enum(
        &mut self,
        name: &str,
        body: &EnumBody,
        indent: usize,
    ) -> Result<(), GeneratorError> {
        if !self.emitted_helpers.insert(name.to_string()) {
            return Ok(());
        }

        line(
            indent,
            &mut self.out,
            format_args!("#[derive(Debug, Clone, Copy, PartialEq, Eq)]"),
        );
        line(indent, &mut self.out, format_args!("#[repr(i32)]"));
        line(indent, &mut self.out, format_args!("pub enum {} {{", name));
        for variant in &body.variants {
            let value = self.render_value(&variant.value);
            line(
                indent + 4,
                &mut self.out,
                format_args!("{} = {},", variant.name, value),
            );
        }
        line(indent, &mut self.out, format_args!("}}"));
        self.out.push('\n');
        Ok(())
    }

    fn emit_named_union(
        &mut self,
        name: &str,
        body: &UnionBody,
        indent: usize,
    ) -> Result<(), GeneratorError> {
        if !self.emitted_helpers.insert(name.to_string()) {
            return Ok(());
        }

        for arm in &body.arms {
            let hint = format!("{}_{}", name, arm.declaration.declarator.name);
            self.emit_inline_helper_types(&arm.declaration.type_spec, &hint, indent)?;
        }

        emit_derive_block(
            indent,
            self.union_is_eq(body),
            &mut self.out,
            DeriveKind::Struct,
        );
        line(indent, &mut self.out, format_args!("pub enum {} {{", name));
        for arm in &body.arms {
            let hint = format!("{}_{}", name, arm.declaration.declarator.name);
            let rust_type = self.rust_type_for_declaration(
                &arm.declaration.type_spec,
                &arm.declaration.declarator,
                &hint,
                indent,
            )?;
            for label in &arm.labels {
                let variant_name = union_variant_name(label);
                if rust_type == "()" {
                    line(indent + 4, &mut self.out, format_args!("{},", variant_name));
                } else {
                    line(
                        indent + 4,
                        &mut self.out,
                        format_args!("{} {{", variant_name),
                    );
                    line(
                        indent + 8,
                        &mut self.out,
                        format_args!("{}: {},", arm.declaration.declarator.name, rust_type),
                    );
                    line(indent + 4, &mut self.out, format_args!("}},"));
                }
            }
        }
        line(indent, &mut self.out, format_args!("}}"));
        self.out.push('\n');
        Ok(())
    }

    fn emit_program(&mut self, item: &ProgramDecl, indent: usize) -> Result<(), GeneratorError> {
        let module_name = to_snake_case(&item.name);
        line(
            indent,
            &mut self.out,
            format_args!("pub mod {} {{", module_name),
        );
        line(
            indent + 4,
            &mut self.out,
            format_args!("pub const PROGRAM: u32 = {};", item.number),
        );
        self.out.push('\n');

        for version in &item.versions {
            let version_module = to_snake_case(&version.name);
            line(
                indent + 4,
                &mut self.out,
                format_args!("pub mod {} {{", version_module),
            );
            line(
                indent + 8,
                &mut self.out,
                format_args!("pub const VERSION: u32 = {};", version.number),
            );
            for procedure in &version.procedures {
                line(
                    indent + 8,
                    &mut self.out,
                    format_args!("pub const {}: u32 = {};", procedure.name, procedure.number),
                );
            }
            line(indent + 4, &mut self.out, format_args!("}}"));
            self.out.push('\n');
        }

        line(indent, &mut self.out, format_args!("}}"));
        self.out.push('\n');
        Ok(())
    }

    fn emit_inline_helper_types(
        &mut self,
        target: &TypeSpec,
        hint: &str,
        indent: usize,
    ) -> Result<(), GeneratorError> {
        match target {
            TypeSpec::Enum(body) => self.emit_named_enum(hint, body, indent),
            TypeSpec::Struct(body) => self.emit_named_struct(hint, body, indent),
            TypeSpec::Union(body) => self.emit_named_union(hint, body, indent),
            _ => Ok(()),
        }
    }

    fn rust_type_for_declaration(
        &mut self,
        target: &TypeSpec,
        declarator: &Declarator,
        hint: &str,
        indent: usize,
    ) -> Result<String, GeneratorError> {
        let base = self.rust_type_for_base(target, hint, indent)?;
        match &declarator.modifier {
            Some(DeclaratorModifier::VariableArray(_)) => match target {
                TypeSpec::Opaque => Ok("Bytes".to_string()),
                TypeSpec::String => Ok("String".to_string()),
                _ => Ok(format!("Vec<{}>", base)),
            },
            Some(DeclaratorModifier::FixedArray(bound)) => match target {
                TypeSpec::Opaque => Ok(format!("[u8; {}]", self.render_array_bound(bound)?)),
                _ => Ok(format!("[{}; {}]", base, self.render_array_bound(bound)?)),
            },
            Some(DeclaratorModifier::Optional) => Ok(format!("Option<Box<{}>>", base)),
            None => match target {
                TypeSpec::Opaque => Ok("u8".to_string()),
                _ => Ok(base),
            },
        }
    }

    fn rust_type_for_base(
        &mut self,
        target: &TypeSpec,
        hint: &str,
        indent: usize,
    ) -> Result<String, GeneratorError> {
        match target {
            TypeSpec::Void => Ok("()".to_string()),
            TypeSpec::Bool => Ok("bool".to_string()),
            TypeSpec::Int => Ok("i32".to_string()),
            TypeSpec::UnsignedInt => Ok("u32".to_string()),
            TypeSpec::Hyper => Ok("i64".to_string()),
            TypeSpec::UnsignedHyper => Ok("u64".to_string()),
            TypeSpec::Float => Ok("f32".to_string()),
            TypeSpec::Double => Ok("f64".to_string()),
            TypeSpec::Quadruple => Ok("[u8; 16]".to_string()),
            TypeSpec::Opaque => Ok("u8".to_string()),
            TypeSpec::String => Ok("String".to_string()),
            TypeSpec::Identifier(name) => Ok(self.qualify_type_name(name)),
            TypeSpec::Enum(body) => {
                self.emit_named_enum(hint, body, indent)?;
                Ok(hint.to_string())
            }
            TypeSpec::Struct(body) => {
                self.emit_named_struct(hint, body, indent)?;
                Ok(hint.to_string())
            }
            TypeSpec::Union(body) => {
                self.emit_named_union(hint, body, indent)?;
                Ok(hint.to_string())
            }
        }
    }

    fn struct_is_eq(&self, body: &StructBody) -> bool {
        self.struct_is_eq_with_seen(body, &mut HashSet::new())
    }

    fn struct_is_eq_with_seen(&self, body: &StructBody, seen: &mut HashSet<String>) -> bool {
        body.declarations.iter().all(|declaration| {
            self.declaration_is_eq_with_seen(
                &declaration.type_spec,
                &declaration.declarator.modifier,
                seen,
            )
        })
    }

    fn union_is_eq(&self, body: &UnionBody) -> bool {
        self.union_is_eq_with_seen(body, &mut HashSet::new())
    }

    fn union_is_eq_with_seen(&self, body: &UnionBody, seen: &mut HashSet<String>) -> bool {
        body.arms.iter().all(|arm| {
            self.declaration_is_eq_with_seen(
                &arm.declaration.type_spec,
                &arm.declaration.declarator.modifier,
                seen,
            )
        })
    }

    fn declaration_is_eq_with_seen(
        &self,
        target: &TypeSpec,
        modifier: &Option<DeclaratorModifier>,
        seen: &mut HashSet<String>,
    ) -> bool {
        self.modifier_is_eq_with_seen(target, modifier, seen)
            && self.type_spec_is_eq_with_seen(target, seen)
    }

    fn modifier_is_eq_with_seen(
        &self,
        target: &TypeSpec,
        modifier: &Option<DeclaratorModifier>,
        seen: &mut HashSet<String>,
    ) -> bool {
        match modifier {
            Some(DeclaratorModifier::VariableArray(_)) => {
                self.type_spec_is_eq_with_seen(target, seen)
            }
            Some(DeclaratorModifier::FixedArray(_)) => self.type_spec_is_eq_with_seen(target, seen),
            Some(DeclaratorModifier::Optional) | None => true,
        }
    }

    fn type_spec_is_eq_with_seen(&self, target: &TypeSpec, seen: &mut HashSet<String>) -> bool {
        match target {
            TypeSpec::Float | TypeSpec::Double => false,
            TypeSpec::Enum(_) => true,
            TypeSpec::Struct(body) => self.struct_is_eq_with_seen(body, seen),
            TypeSpec::Union(body) => self.union_is_eq_with_seen(body, seen),
            TypeSpec::Identifier(name) => self.named_type_is_eq_with_seen(name, seen),
            _ => true,
        }
    }

    fn named_type_is_eq_with_seen(&self, name: &str, seen: &mut HashSet<String>) -> bool {
        if !seen.insert(name.to_string()) {
            return true;
        }

        match self.named_types.get(name) {
            Some(NamedType::Typedef { target, modifier }) => {
                self.declaration_is_eq_with_seen(target, modifier, seen)
            }
            Some(NamedType::Struct(body)) => self.struct_is_eq_with_seen(body, seen),
            Some(NamedType::Enum) => true,
            Some(NamedType::Union(body)) => self.union_is_eq_with_seen(body, seen),
            None => true,
        }
    }

    fn qualify_type_name(&self, name: &str) -> String {
        match (&self.current_module, self.type_owners.get(name)) {
            (Some(current), Some(owner)) if owner != current => {
                format!("crate::{owner}::{name}")
            }
            _ => name.to_string(),
        }
    }

    fn qualify_const_name(&self, name: &str) -> String {
        match (&self.current_module, self.const_owners.get(name)) {
            (Some(current), Some(owner)) if owner != current => {
                format!("crate::{owner}::{name}")
            }
            _ => name.to_string(),
        }
    }

    fn render_value(&self, value: &ValueExpr) -> String {
        match value {
            ValueExpr::Number(value) => value.to_string(),
            ValueExpr::Identifier(name) => self.qualify_const_name(name),
        }
    }

    fn render_array_bound(&self, value: &ValueExpr) -> Result<String, GeneratorError> {
        match value {
            ValueExpr::Number(value) if *value >= 0 => Ok(format!("{}usize", value)),
            ValueExpr::Number(value) => Err(GeneratorError::UnsupportedConstruct(format!(
                "negative fixed array bounds are not supported in Rust emission: {value}"
            ))),
            ValueExpr::Identifier(name) => {
                Ok(format!("{} as usize", self.qualify_const_name(name)))
            }
        }
    }
}

enum DeriveKind {
    Struct,
}

fn emit_derive_block(indent: usize, is_eq: bool, out: &mut String, kind: DeriveKind) {
    match kind {
        DeriveKind::Struct if is_eq => line(
            indent,
            out,
            format_args!("#[derive(Debug, Clone, PartialEq, Eq)]"),
        ),
        DeriveKind::Struct => line(
            indent,
            out,
            format_args!("#[derive(Debug, Clone, PartialEq)]"),
        ),
    }
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

fn schema_uses_bytes(schema: &Schema) -> bool {
    schema.items.iter().any(item_uses_bytes)
}

fn item_uses_bytes(item: &Item) -> bool {
    match item {
        Item::Const(_) | Item::Program(_) => false,
        Item::Typedef(item) => declaration_uses_bytes(&item.target, &item.declarator),
        Item::Struct(item) => item
            .body
            .declarations
            .iter()
            .any(|field| declaration_uses_bytes(&field.type_spec, &field.declarator)),
        Item::Enum(_) => false,
        Item::Union(item) => item.body.arms.iter().any(|arm| {
            declaration_uses_bytes(&arm.declaration.type_spec, &arm.declaration.declarator)
        }),
    }
}

fn declaration_uses_bytes(target: &TypeSpec, declarator: &Declarator) -> bool {
    uses_bytes_in_type_spec(target)
        || matches!(
            (&declarator.modifier, target),
            (Some(DeclaratorModifier::VariableArray(_)), TypeSpec::Opaque)
        )
}

fn uses_bytes_in_type_spec(target: &TypeSpec) -> bool {
    match target {
        TypeSpec::Enum(_) => false,
        TypeSpec::Struct(body) => body.declarations.iter().any(|declaration| {
            declaration_uses_bytes(&declaration.type_spec, &declaration.declarator)
        }),
        TypeSpec::Union(body) => body.arms.iter().any(|arm| {
            declaration_uses_bytes(&arm.declaration.type_spec, &arm.declaration.declarator)
        }),
        _ => false,
    }
}

fn union_variant_name(label: &UnionCaseLabel) -> String {
    match label {
        UnionCaseLabel::Case(ValueExpr::Identifier(name)) => to_pascal_case(name),
        UnionCaseLabel::Case(ValueExpr::Number(value)) if *value >= 0 => format!("Case{value}"),
        UnionCaseLabel::Case(ValueExpr::Number(value)) => {
            format!("CaseNeg{}", value.unsigned_abs())
        }
        UnionCaseLabel::Default => "Default".to_string(),
    }
}

fn to_snake_case(input: &str) -> String {
    input.to_ascii_lowercase()
}

fn to_pascal_case(input: &str) -> String {
    let mut out = String::new();
    let mut uppercase_next = true;

    for ch in input.chars() {
        if ch == '_' || ch == '-' {
            uppercase_next = true;
            continue;
        }
        if uppercase_next {
            out.extend(ch.to_uppercase());
            uppercase_next = false;
        } else {
            out.push(ch.to_ascii_lowercase());
        }
    }

    if out.is_empty() {
        "Variant".to_string()
    } else {
        out
    }
}

fn line(indent: usize, out: &mut String, args: std::fmt::Arguments<'_>) {
    let _ = write!(out, "{:indent$}", "", indent = indent);
    let _ = out.write_fmt(args);
    out.push('\n');
}

fn build_named_types(schema: &Schema) -> BTreeMap<String, NamedType> {
    build_named_types_for_modules(&[LoadedModule {
        module_name: "__local".to_string(),
        path: Default::default(),
        dependencies: Vec::new(),
        schema: schema.clone(),
    }])
}

fn build_named_types_for_modules(modules: &[LoadedModule]) -> BTreeMap<String, NamedType> {
    let mut named_types = BTreeMap::new();

    for module in modules {
        for item in &module.schema.items {
            match item {
                Item::Typedef(item) => {
                    named_types.insert(
                        item.declarator.name.clone(),
                        NamedType::Typedef {
                            target: item.target.clone(),
                            modifier: item.declarator.modifier.clone(),
                        },
                    );
                }
                Item::Struct(item) => {
                    named_types.insert(item.name.clone(), NamedType::Struct(item.body.clone()));
                }
                Item::Enum(item) => {
                    named_types.insert(item.name.clone(), NamedType::Enum);
                }
                Item::Union(item) => {
                    named_types.insert(item.name.clone(), NamedType::Union(item.body.clone()));
                }
                Item::Const(_) | Item::Program(_) => {}
            }
        }
    }

    named_types
}

fn build_type_owners(modules: &[LoadedModule]) -> BTreeMap<String, String> {
    let mut owners = BTreeMap::new();

    for module in modules {
        for item in &module.schema.items {
            match item {
                Item::Typedef(item) => {
                    owners.insert(item.declarator.name.clone(), module.module_name.clone());
                }
                Item::Struct(item) => {
                    owners.insert(item.name.clone(), module.module_name.clone());
                }
                Item::Enum(item) => {
                    owners.insert(item.name.clone(), module.module_name.clone());
                }
                Item::Union(item) => {
                    owners.insert(item.name.clone(), module.module_name.clone());
                }
                Item::Const(_) | Item::Program(_) => {}
            }
        }
    }

    owners
}

fn build_const_owners(modules: &[LoadedModule]) -> BTreeMap<String, String> {
    let mut owners = BTreeMap::new();

    for module in modules {
        for item in &module.schema.items {
            if let Item::Const(item) = item {
                owners.insert(item.name.clone(), module.module_name.clone());
            }
        }
    }

    owners
}
