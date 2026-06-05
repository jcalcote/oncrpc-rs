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
    let mut emitter = StubEmitter::new(schema, true, true);
    emitter.emit_schema(schema)?;
    Ok(emitter.out)
}

pub fn emit_rust_stubs_with_options(
    schema: &Schema,
    emit_client: bool,
    emit_server: bool,
) -> Result<String, GeneratorError> {
    let mut emitter = StubEmitter::new(schema, emit_client, emit_server);
    emitter.emit_schema(schema)?;
    Ok(emitter.out)
}

pub fn emit_rust_stubs_for_module(
    module: &LoadedModule,
    loaded: &LoadedSchemaSet,
) -> Result<String, GeneratorError> {
    let mut emitter = StubEmitter::new_for_module(module, loaded, true, true);
    emitter.emit_schema(&module.schema)?;
    Ok(emitter.out)
}

pub fn emit_rust_stubs_for_module_with_options(
    module: &LoadedModule,
    loaded: &LoadedSchemaSet,
    emit_client: bool,
    emit_server: bool,
) -> Result<String, GeneratorError> {
    let mut emitter = StubEmitter::new_for_module(module, loaded, emit_client, emit_server);
    emitter.emit_schema(&module.schema)?;
    Ok(emitter.out)
}

struct StubEmitter {
    out: String,
    current_module: Option<String>,
    type_owners: BTreeMap<String, String>,
    emit_client: bool,
    emit_server: bool,
}

impl StubEmitter {
    fn new(schema: &Schema, emit_client: bool, emit_server: bool) -> Self {
        Self {
            out: String::new(),
            current_module: None,
            type_owners: build_type_owners(&[LoadedModule {
                module_name: "__local".to_string(),
                path: Default::default(),
                dependencies: Vec::new(),
                schema: schema.clone(),
            }]),
            emit_client,
            emit_server,
        }
    }

    fn new_for_module(
        module: &LoadedModule,
        loaded: &LoadedSchemaSet,
        emit_client: bool,
        emit_server: bool,
    ) -> Self {
        Self {
            out: String::new(),
            current_module: Some(module.module_name.clone()),
            type_owners: build_type_owners(&loaded.modules),
            emit_client,
            emit_server,
        }
    }

    fn emit_schema(&mut self, schema: &Schema) -> Result<(), GeneratorError> {
        for item in &schema.items {
            if let Item::Program(program) = item {
                self.emit_program(program, 0)?;
            }
        }
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
            self.emit_version(version, indent + 4)?;
        }

        line(indent, &mut self.out, format_args!("}}"));
        self.out.push('\n');
        Ok(())
    }

    fn emit_version(&mut self, version: &VersionDecl, indent: usize) -> Result<(), GeneratorError> {
        let module_name = to_snake_case(&version.name);
        let client_name = format!("{}Client", version.name);
        let service_name = format!("{}Service", version.name);
        let dispatch_name = format!("{}Dispatch", version.name);

        line(
            indent,
            &mut self.out,
            format_args!("pub mod {} {{", module_name),
        );
        line(
            indent + 4,
            &mut self.out,
            format_args!("pub const VERSION: u32 = {};", version.number),
        );
        for procedure in &version.procedures {
            line(
                indent + 4,
                &mut self.out,
                format_args!("pub const {}: u32 = {};", procedure.name, procedure.number),
            );
        }
        self.out.push('\n');

        if self.emit_client {
            line(indent + 4, &mut self.out, format_args!("pub mod client {{"));
            line(
                indent + 8,
                &mut self.out,
                format_args!("pub struct {}<T> {{", client_name),
            );
            line(
                indent + 12,
                &mut self.out,
                format_args!("client: onc_rpc_runtime::Client<T>,"),
            );
            line(indent + 8, &mut self.out, format_args!("}}"));
            self.out.push('\n');

            line(
                indent + 8,
                &mut self.out,
                format_args!(
                    "impl<T> {}<T> where T: onc_rpc_runtime::ClientTransport {{",
                    client_name
                ),
            );
            line(
                indent + 12,
                &mut self.out,
                format_args!("pub fn new(client: onc_rpc_runtime::Client<T>) -> Self {{"),
            );
            line(
                indent + 16,
                &mut self.out,
                format_args!("Self {{ client }}"),
            );
            line(indent + 12, &mut self.out, format_args!("}}"));
            self.out.push('\n');

            for procedure in &version.procedures {
                self.emit_client_method(procedure, indent + 12)?;
            }

            line(indent + 8, &mut self.out, format_args!("}}"));
            self.out.push('\n');
            line(indent + 4, &mut self.out, format_args!("}}"));
            self.out.push('\n');

            line(
                indent + 4,
                &mut self.out,
                format_args!("pub mod async_client {{"),
            );
            line(
                indent + 8,
                &mut self.out,
                format_args!("pub struct {}<T> {{", client_name),
            );
            line(
                indent + 12,
                &mut self.out,
                format_args!("client: onc_rpc_runtime::AsyncClient<T>,"),
            );
            line(indent + 8, &mut self.out, format_args!("}}"));
            self.out.push('\n');

            line(
                indent + 8,
                &mut self.out,
                format_args!(
                    "impl<T> {}<T> where T: onc_rpc_runtime::AsyncClientTransport {{",
                    client_name
                ),
            );
            line(
                indent + 12,
                &mut self.out,
                format_args!("pub fn new(client: onc_rpc_runtime::AsyncClient<T>) -> Self {{"),
            );
            line(
                indent + 16,
                &mut self.out,
                format_args!("Self {{ client }}"),
            );
            line(indent + 12, &mut self.out, format_args!("}}"));
            self.out.push('\n');

            for procedure in &version.procedures {
                self.emit_async_client_method(procedure, indent + 12)?;
            }

            line(indent + 8, &mut self.out, format_args!("}}"));
            self.out.push('\n');
            line(indent + 4, &mut self.out, format_args!("}}"));
            self.out.push('\n');
        }

        if self.emit_server {
            line(indent + 4, &mut self.out, format_args!("pub mod server {{"));
            line(
                indent + 8,
                &mut self.out,
                format_args!("pub trait {} {{", service_name),
            );
            for procedure in &version.procedures {
                self.emit_service_method(procedure, indent + 12)?;
            }
            line(indent + 8, &mut self.out, format_args!("}}"));
            self.out.push('\n');

            line(
                indent + 8,
                &mut self.out,
                format_args!("pub struct {}<T> {{", dispatch_name),
            );
            line(indent + 12, &mut self.out, format_args!("inner: T,"));
            line(indent + 8, &mut self.out, format_args!("}}"));
            self.out.push('\n');

            line(
                indent + 8,
                &mut self.out,
                format_args!("impl<T> {}<T> {{", dispatch_name),
            );
            line(
                indent + 12,
                &mut self.out,
                format_args!("pub fn new(inner: T) -> Self {{"),
            );
            line(indent + 16, &mut self.out, format_args!("Self {{ inner }}"));
            line(indent + 12, &mut self.out, format_args!("}}"));
            line(indent + 8, &mut self.out, format_args!("}}"));
            self.out.push('\n');

            line(
                indent + 8,
                &mut self.out,
                format_args!(
                    "impl<T> onc_rpc_server::Dispatch for {}<T> where T: {} + Send + Sync + 'static {{",
                    dispatch_name, service_name
                ),
            );
            line(
                indent + 12,
                &mut self.out,
                format_args!(
                    "fn dispatch(&self, request: onc_rpc_server::RequestContext) -> Result<onc_rpc_server::ResponsePayload, onc_rpc_server::DispatchError> {{"
                ),
            );
            line(
                indent + 16,
                &mut self.out,
                format_args!("match request.procedure.0 {{"),
            );
            for procedure in &version.procedures {
                self.emit_dispatch_arm(procedure, indent + 20)?;
            }
            line(
                indent + 20,
                &mut self.out,
                format_args!("_ => Err(onc_rpc_server::DispatchError::ProcedureUnavailable),"),
            );
            line(indent + 16, &mut self.out, format_args!("}}"));
            line(indent + 12, &mut self.out, format_args!("}}"));
            line(indent + 8, &mut self.out, format_args!("}}"));
            self.out.push('\n');
            line(indent + 4, &mut self.out, format_args!("}}"));
            self.out.push('\n');

            line(
                indent + 4,
                &mut self.out,
                format_args!("pub mod async_server {{"),
            );
            line(
                indent + 8,
                &mut self.out,
                format_args!("#[onc_rpc_server::async_trait]"),
            );
            line(
                indent + 8,
                &mut self.out,
                format_args!("pub trait {} {{", service_name),
            );
            for procedure in &version.procedures {
                self.emit_async_service_method(procedure, indent + 12)?;
            }
            line(indent + 8, &mut self.out, format_args!("}}"));
            self.out.push('\n');

            line(
                indent + 8,
                &mut self.out,
                format_args!("pub struct {}<T> {{", dispatch_name),
            );
            line(indent + 12, &mut self.out, format_args!("inner: T,"));
            line(indent + 8, &mut self.out, format_args!("}}"));
            self.out.push('\n');

            line(
                indent + 8,
                &mut self.out,
                format_args!("impl<T> {}<T> {{", dispatch_name),
            );
            line(
                indent + 12,
                &mut self.out,
                format_args!("pub fn new(inner: T) -> Self {{"),
            );
            line(indent + 16, &mut self.out, format_args!("Self {{ inner }}"));
            line(indent + 12, &mut self.out, format_args!("}}"));
            line(indent + 8, &mut self.out, format_args!("}}"));
            self.out.push('\n');

            line(
                indent + 8,
                &mut self.out,
                format_args!("#[onc_rpc_server::async_trait]"),
            );
            line(
                indent + 8,
                &mut self.out,
                format_args!(
                    "impl<T> onc_rpc_server::AsyncDispatch for {}<T> where T: {} + Send + Sync + 'static {{",
                    dispatch_name, service_name
                ),
            );
            line(
                indent + 12,
                &mut self.out,
                format_args!(
                    "async fn dispatch(&self, request: onc_rpc_server::RequestContext) -> Result<onc_rpc_server::ResponsePayload, onc_rpc_server::DispatchError> {{"
                ),
            );
            line(
                indent + 16,
                &mut self.out,
                format_args!("match request.procedure.0 {{"),
            );
            for procedure in &version.procedures {
                self.emit_async_dispatch_arm(procedure, indent + 20)?;
            }
            line(
                indent + 20,
                &mut self.out,
                format_args!("_ => Err(onc_rpc_server::DispatchError::ProcedureUnavailable),"),
            );
            line(indent + 16, &mut self.out, format_args!("}}"));
            line(indent + 12, &mut self.out, format_args!("}}"));
            line(indent + 8, &mut self.out, format_args!("}}"));
            self.out.push('\n');
            line(indent + 4, &mut self.out, format_args!("}}"));
            self.out.push('\n');
        }

        line(indent, &mut self.out, format_args!("}}"));
        self.out.push('\n');

        Ok(())
    }

    fn emit_client_method(
        &mut self,
        procedure: &ProcedureDecl,
        indent: usize,
    ) -> Result<(), GeneratorError> {
        let method_name = to_snake_case(&procedure.name);
        let request_type = self.procedure_rust_type(
            &procedure.argument_type,
            &format!("{}_arg", method_name),
            indent,
        )?;
        let reply_type = self.procedure_rust_type(
            &procedure.return_type,
            &format!("{}_ret", method_name),
            indent,
        )?;

        if request_type == "()" {
            line(
                indent,
                &mut self.out,
                format_args!(
                    "pub fn {}(&self) -> Result<{}, onc_rpc_runtime::RuntimeError> {{",
                    method_name, reply_type
                ),
            );
            line(
                indent + 4,
                &mut self.out,
                format_args!("self.client.call_typed("),
            );
            line(
                indent + 8,
                &mut self.out,
                format_args!(
                    "onc_rpc_runtime::ProgramVersion {{ program: super::super::PROGRAM, version: super::VERSION }},"
                ),
            );
            line(
                indent + 8,
                &mut self.out,
                format_args!("onc_rpc_runtime::Procedure(super::{}),", procedure.name),
            );
            line(indent + 8, &mut self.out, format_args!("&(),"));
            line(indent + 4, &mut self.out, format_args!(")"));
            line(indent, &mut self.out, format_args!("}}"));
            self.out.push('\n');

            line(
                indent,
                &mut self.out,
                format_args!(
                    "pub fn {}_with_options(&self, options: &onc_rpc_runtime::CallOptions) -> Result<{}, onc_rpc_runtime::RuntimeError> {{",
                    method_name, reply_type
                ),
            );
            line(
                indent + 4,
                &mut self.out,
                format_args!("self.client.call_typed_with_options("),
            );
            line(
                indent + 8,
                &mut self.out,
                format_args!(
                    "onc_rpc_runtime::ProgramVersion {{ program: super::super::PROGRAM, version: super::VERSION }},"
                ),
            );
            line(
                indent + 8,
                &mut self.out,
                format_args!("onc_rpc_runtime::Procedure(super::{}),", procedure.name),
            );
            line(indent + 8, &mut self.out, format_args!("&(),"));
            line(indent + 8, &mut self.out, format_args!("options,"));
            line(indent + 4, &mut self.out, format_args!(")"));
        } else {
            line(
                indent,
                &mut self.out,
                format_args!(
                    "pub fn {}(&self, argument: {}) -> Result<{}, onc_rpc_runtime::RuntimeError> {{",
                    method_name, request_type, reply_type
                ),
            );
            line(
                indent + 4,
                &mut self.out,
                format_args!("self.client.call_typed("),
            );
            line(
                indent + 8,
                &mut self.out,
                format_args!(
                    "onc_rpc_runtime::ProgramVersion {{ program: super::super::PROGRAM, version: super::VERSION }},"
                ),
            );
            line(
                indent + 8,
                &mut self.out,
                format_args!("onc_rpc_runtime::Procedure(super::{}),", procedure.name),
            );
            line(indent + 8, &mut self.out, format_args!("&argument,"));
            line(indent + 4, &mut self.out, format_args!(")"));
            line(indent, &mut self.out, format_args!("}}"));
            self.out.push('\n');

            line(
                indent,
                &mut self.out,
                format_args!(
                    "pub fn {}_with_options(&self, argument: {}, options: &onc_rpc_runtime::CallOptions) -> Result<{}, onc_rpc_runtime::RuntimeError> {{",
                    method_name, request_type, reply_type
                ),
            );
            line(
                indent + 4,
                &mut self.out,
                format_args!("self.client.call_typed_with_options("),
            );
            line(
                indent + 8,
                &mut self.out,
                format_args!(
                    "onc_rpc_runtime::ProgramVersion {{ program: super::super::PROGRAM, version: super::VERSION }},"
                ),
            );
            line(
                indent + 8,
                &mut self.out,
                format_args!("onc_rpc_runtime::Procedure(super::{}),", procedure.name),
            );
            line(indent + 8, &mut self.out, format_args!("&argument,"));
            line(indent + 8, &mut self.out, format_args!("options,"));
            line(indent + 4, &mut self.out, format_args!(")"));
        }
        line(indent, &mut self.out, format_args!("}}"));
        self.out.push('\n');
        Ok(())
    }

    fn emit_async_client_method(
        &mut self,
        procedure: &ProcedureDecl,
        indent: usize,
    ) -> Result<(), GeneratorError> {
        let method_name = to_snake_case(&procedure.name);
        let request_type = self.procedure_rust_type(
            &procedure.argument_type,
            &format!("{}_arg", method_name),
            indent,
        )?;
        let reply_type = self.procedure_rust_type(
            &procedure.return_type,
            &format!("{}_ret", method_name),
            indent,
        )?;

        if request_type == "()" {
            line(
                indent,
                &mut self.out,
                format_args!(
                    "pub async fn {}(&self) -> Result<{}, onc_rpc_runtime::RuntimeError> {{",
                    method_name, reply_type
                ),
            );
            line(
                indent + 4,
                &mut self.out,
                format_args!("self.client.call_typed("),
            );
            line(
                indent + 8,
                &mut self.out,
                format_args!(
                    "onc_rpc_runtime::ProgramVersion {{ program: super::super::PROGRAM, version: super::VERSION }},"
                ),
            );
            line(
                indent + 8,
                &mut self.out,
                format_args!("onc_rpc_runtime::Procedure(super::{}),", procedure.name),
            );
            line(indent + 8, &mut self.out, format_args!("&(),"));
            line(indent + 4, &mut self.out, format_args!(").await"));
            line(indent, &mut self.out, format_args!("}}"));
            self.out.push('\n');

            line(
                indent,
                &mut self.out,
                format_args!(
                    "pub async fn {}_with_options(&self, options: &onc_rpc_runtime::CallOptions) -> Result<{}, onc_rpc_runtime::RuntimeError> {{",
                    method_name, reply_type
                ),
            );
            line(
                indent + 4,
                &mut self.out,
                format_args!("self.client.call_typed_with_options("),
            );
            line(
                indent + 8,
                &mut self.out,
                format_args!(
                    "onc_rpc_runtime::ProgramVersion {{ program: super::super::PROGRAM, version: super::VERSION }},"
                ),
            );
            line(
                indent + 8,
                &mut self.out,
                format_args!("onc_rpc_runtime::Procedure(super::{}),", procedure.name),
            );
            line(indent + 8, &mut self.out, format_args!("&(),"));
            line(indent + 8, &mut self.out, format_args!("options,"));
            line(indent + 4, &mut self.out, format_args!(").await"));
        } else {
            line(
                indent,
                &mut self.out,
                format_args!(
                    "pub async fn {}(&self, argument: {}) -> Result<{}, onc_rpc_runtime::RuntimeError> {{",
                    method_name, request_type, reply_type
                ),
            );
            line(
                indent + 4,
                &mut self.out,
                format_args!("self.client.call_typed("),
            );
            line(
                indent + 8,
                &mut self.out,
                format_args!(
                    "onc_rpc_runtime::ProgramVersion {{ program: super::super::PROGRAM, version: super::VERSION }},"
                ),
            );
            line(
                indent + 8,
                &mut self.out,
                format_args!("onc_rpc_runtime::Procedure(super::{}),", procedure.name),
            );
            line(indent + 8, &mut self.out, format_args!("&argument,"));
            line(indent + 4, &mut self.out, format_args!(").await"));
            line(indent, &mut self.out, format_args!("}}"));
            self.out.push('\n');

            line(
                indent,
                &mut self.out,
                format_args!(
                    "pub async fn {}_with_options(&self, argument: {}, options: &onc_rpc_runtime::CallOptions) -> Result<{}, onc_rpc_runtime::RuntimeError> {{",
                    method_name, request_type, reply_type
                ),
            );
            line(
                indent + 4,
                &mut self.out,
                format_args!("self.client.call_typed_with_options("),
            );
            line(
                indent + 8,
                &mut self.out,
                format_args!(
                    "onc_rpc_runtime::ProgramVersion {{ program: super::super::PROGRAM, version: super::VERSION }},"
                ),
            );
            line(
                indent + 8,
                &mut self.out,
                format_args!("onc_rpc_runtime::Procedure(super::{}),", procedure.name),
            );
            line(indent + 8, &mut self.out, format_args!("&argument,"));
            line(indent + 8, &mut self.out, format_args!("options,"));
            line(indent + 4, &mut self.out, format_args!(").await"));
        }
        line(indent, &mut self.out, format_args!("}}"));
        self.out.push('\n');
        Ok(())
    }

    fn emit_service_method(
        &mut self,
        procedure: &ProcedureDecl,
        indent: usize,
    ) -> Result<(), GeneratorError> {
        let method_name = to_snake_case(&procedure.name);
        let request_type = self.procedure_rust_type(
            &procedure.argument_type,
            &format!("{}_arg", method_name),
            indent,
        )?;
        let reply_type = self.procedure_rust_type(
            &procedure.return_type,
            &format!("{}_ret", method_name),
            indent,
        )?;

        if request_type == "()" {
            line(
                indent,
                &mut self.out,
                format_args!(
                    "fn {}(&self, request: &onc_rpc_server::RequestContext) -> Result<{}, onc_rpc_server::DispatchError>;",
                    method_name, reply_type
                ),
            );
        } else {
            line(
                indent,
                &mut self.out,
                format_args!(
                    "fn {}(&self, request: &onc_rpc_server::RequestContext, argument: {}) -> Result<{}, onc_rpc_server::DispatchError>;",
                    method_name, request_type, reply_type
                ),
            );
        }
        Ok(())
    }

    fn emit_async_service_method(
        &mut self,
        procedure: &ProcedureDecl,
        indent: usize,
    ) -> Result<(), GeneratorError> {
        let method_name = to_snake_case(&procedure.name);
        let request_type = self.procedure_rust_type(
            &procedure.argument_type,
            &format!("{}_arg", method_name),
            indent,
        )?;
        let reply_type = self.procedure_rust_type(
            &procedure.return_type,
            &format!("{}_ret", method_name),
            indent,
        )?;

        if request_type == "()" {
            line(
                indent,
                &mut self.out,
                format_args!(
                    "async fn {}(&self, request: &onc_rpc_server::RequestContext) -> Result<{}, onc_rpc_server::DispatchError>;",
                    method_name, reply_type
                ),
            );
        } else {
            line(
                indent,
                &mut self.out,
                format_args!(
                    "async fn {}(&self, request: &onc_rpc_server::RequestContext, argument: {}) -> Result<{}, onc_rpc_server::DispatchError>;",
                    method_name, request_type, reply_type
                ),
            );
        }
        Ok(())
    }

    fn emit_dispatch_arm(
        &mut self,
        procedure: &ProcedureDecl,
        indent: usize,
    ) -> Result<(), GeneratorError> {
        let method_name = to_snake_case(&procedure.name);
        let request_type = self.procedure_rust_type(
            &procedure.argument_type,
            &format!("{}_arg", method_name),
            indent,
        )?;
        let reply_type = self.procedure_rust_type(
            &procedure.return_type,
            &format!("{}_ret", method_name),
            indent,
        )?;

        line(
            indent,
            &mut self.out,
            format_args!("{} => {{", procedure.number),
        );
        if request_type == "()" {
            line(
                indent + 4,
                &mut self.out,
                format_args!(
                    "<() as onc_rpc_xdr::XdrDecode>::from_xdr_bytes(&request.payload)\
                        .map_err(|_| onc_rpc_server::DispatchError::GarbageArgs)?;"
                ),
            );
            if reply_type == "()" {
                line(
                    indent + 4,
                    &mut self.out,
                    format_args!("self.inner.{}(&request)?;", method_name),
                );
            } else {
                line(
                    indent + 4,
                    &mut self.out,
                    format_args!("let response = self.inner.{}(&request)?;", method_name),
                );
            }
        } else {
            line(
                indent + 4,
                &mut self.out,
                format_args!(
                    "let argument = <{} as onc_rpc_xdr::XdrDecode>::from_xdr_bytes(&request.payload)\
                        .map_err(|_| onc_rpc_server::DispatchError::GarbageArgs)?;",
                    request_type
                ),
            );
            if reply_type == "()" {
                line(
                    indent + 4,
                    &mut self.out,
                    format_args!("self.inner.{}(&request, argument)?;", method_name),
                );
            } else {
                line(
                    indent + 4,
                    &mut self.out,
                    format_args!(
                        "let response = self.inner.{}(&request, argument)?;",
                        method_name
                    ),
                );
            }
        }
        if reply_type == "()" {
            line(
                indent + 4,
                &mut self.out,
                format_args!(
                    "let payload = onc_rpc_xdr::XdrEncode::to_xdr_bytes(&())\
                        .map_err(|_| onc_rpc_server::DispatchError::SystemError)?;"
                ),
            );
        } else {
            line(
                indent + 4,
                &mut self.out,
                format_args!(
                    "let payload = onc_rpc_xdr::XdrEncode::to_xdr_bytes(&response)\
                        .map_err(|_| onc_rpc_server::DispatchError::SystemError)?;"
                ),
            );
        }
        line(
            indent + 4,
            &mut self.out,
            format_args!("Ok(onc_rpc_server::ResponsePayload::success(payload))"),
        );
        line(indent, &mut self.out, format_args!("}}"));
        Ok(())
    }

    fn emit_async_dispatch_arm(
        &mut self,
        procedure: &ProcedureDecl,
        indent: usize,
    ) -> Result<(), GeneratorError> {
        let method_name = to_snake_case(&procedure.name);
        let request_type = self.procedure_rust_type(
            &procedure.argument_type,
            &format!("{}_arg", method_name),
            indent,
        )?;
        let reply_type = self.procedure_rust_type(
            &procedure.return_type,
            &format!("{}_ret", method_name),
            indent,
        )?;

        line(
            indent,
            &mut self.out,
            format_args!("{} => {{", procedure.number),
        );
        if request_type == "()" {
            line(
                indent + 4,
                &mut self.out,
                format_args!(
                    "<() as onc_rpc_xdr::XdrDecode>::from_xdr_bytes(&request.payload)\
                        .map_err(|_| onc_rpc_server::DispatchError::GarbageArgs)?;"
                ),
            );
            if reply_type == "()" {
                line(
                    indent + 4,
                    &mut self.out,
                    format_args!("self.inner.{}(&request).await?;", method_name),
                );
            } else {
                line(
                    indent + 4,
                    &mut self.out,
                    format_args!(
                        "let response = self.inner.{}(&request).await?;",
                        method_name
                    ),
                );
            }
        } else {
            line(
                indent + 4,
                &mut self.out,
                format_args!(
                    "let argument = <{} as onc_rpc_xdr::XdrDecode>::from_xdr_bytes(&request.payload)\
                        .map_err(|_| onc_rpc_server::DispatchError::GarbageArgs)?;",
                    request_type
                ),
            );
            if reply_type == "()" {
                line(
                    indent + 4,
                    &mut self.out,
                    format_args!("self.inner.{}(&request, argument).await?;", method_name),
                );
            } else {
                line(
                    indent + 4,
                    &mut self.out,
                    format_args!(
                        "let response = self.inner.{}(&request, argument).await?;",
                        method_name
                    ),
                );
            }
        }
        if reply_type == "()" {
            line(
                indent + 4,
                &mut self.out,
                format_args!(
                    "let payload = onc_rpc_xdr::XdrEncode::to_xdr_bytes(&())\
                        .map_err(|_| onc_rpc_server::DispatchError::SystemError)?;"
                ),
            );
        } else {
            line(
                indent + 4,
                &mut self.out,
                format_args!(
                    "let payload = onc_rpc_xdr::XdrEncode::to_xdr_bytes(&response)\
                        .map_err(|_| onc_rpc_server::DispatchError::SystemError)?;"
                ),
            );
        }
        line(
            indent + 4,
            &mut self.out,
            format_args!("Ok(onc_rpc_server::ResponsePayload::success(payload))"),
        );
        line(indent, &mut self.out, format_args!("}}"));
        Ok(())
    }

    fn procedure_rust_type(
        &self,
        target: &TypeSpec,
        hint: &str,
        indent: usize,
    ) -> Result<String, GeneratorError> {
        let _ = (hint, indent);
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
            TypeSpec::Enum(_) | TypeSpec::Struct(_) | TypeSpec::Union(_) => {
                Err(GeneratorError::UnsupportedConstruct(
                    "inline program procedure types are not yet supported in stub emission"
                        .to_string(),
                ))
            }
        }
    }

    fn qualify_type_name(&self, name: &str) -> String {
        match (&self.current_module, self.type_owners.get(name)) {
            (Some(_), Some(owner)) => {
                format!("crate::{owner}::{name}")
            }
            _ => name.to_string(),
        }
    }
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
            out.push_str("use bytes::Bytes;\n");
        }
        if schema_uses_xdr_traits(schema) {
            out.push_str("use onc_rpc_xdr::{XdrDecode, XdrEncode};\n");
        }
        if schema_uses_bytes(schema) || schema_uses_xdr_traits(schema) {
            out.push('\n');
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
            out.push_str("use bytes::Bytes;\n");
        }
        if schema_uses_xdr_traits(&module.schema) {
            out.push_str("use onc_rpc_xdr::{XdrDecode, XdrEncode};\n");
        }
        if schema_uses_bytes(&module.schema) || schema_uses_xdr_traits(&module.schema) {
            out.push('\n');
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
            format_args!("pub const {}: i64 = {};", rust_ident(&item.name), value),
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
                    format_args!(
                        "pub type {} = {};",
                        rust_ident(&item.declarator.name),
                        rust_type
                    ),
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
                    format_args!(
                        "pub type {} = {};",
                        rust_ident(&item.declarator.name),
                        rust_type
                    ),
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
                    format_args!(
                        "pub type {} = {};",
                        rust_ident(&item.declarator.name),
                        rust_type
                    ),
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
                    format_args!(
                        "pub type {} = {};",
                        rust_ident(&item.declarator.name),
                        rust_type
                    ),
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
                format_args!(
                    "pub {}: {},",
                    rust_ident(&declaration.declarator.name),
                    rust_type
                ),
            );
        }
        line(indent, &mut self.out, format_args!("}}"));
        self.out.push('\n');
        self.emit_struct_xdr_impl(name, body, indent)?;
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
        self.emit_enum_xdr_impl(name, body, indent);
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
        let discriminant_type = self.rust_type_for_declaration(
            &body.discriminant.type_spec,
            &body.discriminant.declarator,
            &format!("{name}_discriminant"),
            indent,
        )?;
        let default_discriminant_type =
            self.union_default_discriminant_type(&body.discriminant.type_spec, &discriminant_type);
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
                if matches!(label, UnionCaseLabel::Default) {
                    line(
                        indent + 4,
                        &mut self.out,
                        format_args!("{} {{", variant_name),
                    );
                    line(
                        indent + 8,
                        &mut self.out,
                        format_args!("discriminant: {},", default_discriminant_type),
                    );
                    if rust_type != "()" {
                        line(
                            indent + 8,
                            &mut self.out,
                            format_args!(
                                "{}: {},",
                                rust_ident(&arm.declaration.declarator.name),
                                rust_type
                            ),
                        );
                    }
                    line(indent + 4, &mut self.out, format_args!("}},"));
                } else if rust_type == "()" {
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
                        format_args!(
                            "{}: {},",
                            rust_ident(&arm.declaration.declarator.name),
                            rust_type
                        ),
                    );
                    line(indent + 4, &mut self.out, format_args!("}},"));
                }
            }
        }
        line(indent, &mut self.out, format_args!("}}"));
        self.out.push('\n');
        self.emit_union_xdr_impl(name, body, &discriminant_type, indent)?;
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

    fn emit_struct_xdr_impl(
        &mut self,
        name: &str,
        body: &StructBody,
        indent: usize,
    ) -> Result<(), GeneratorError> {
        line(
            indent,
            &mut self.out,
            format_args!("impl XdrEncode for {} {{", name),
        );
        line(
            indent + 4,
            &mut self.out,
            format_args!(
                "fn encode_xdr(&self, output: &mut bytes::BytesMut) -> Result<(), onc_rpc_xdr::XdrError> {{"
            ),
        );
        for declaration in &body.declarations {
            let field_name = rust_ident(&declaration.declarator.name);
            self.emit_encode_declaration(
                &declaration.type_spec,
                &declaration.declarator,
                &format!("self.{field_name}"),
                indent + 8,
            )?;
        }
        line(indent + 8, &mut self.out, format_args!("Ok(())"));
        line(indent + 4, &mut self.out, format_args!("}}"));
        line(indent, &mut self.out, format_args!("}}"));
        self.out.push('\n');

        line(
            indent,
            &mut self.out,
            format_args!("impl XdrDecode for {} {{", name),
        );
        line(
            indent + 4,
            &mut self.out,
            format_args!(
                "fn decode_xdr(input: &mut &[u8]) -> Result<Self, onc_rpc_xdr::XdrError> {{"
            ),
        );
        line(indent + 8, &mut self.out, format_args!("Ok(Self {{"));
        for declaration in &body.declarations {
            let hint = format!("{}_{}", name, declaration.declarator.name);
            let decode = self.decode_declaration_expr(
                &declaration.type_spec,
                &declaration.declarator,
                &hint,
                indent + 8,
            )?;
            line(
                indent + 12,
                &mut self.out,
                format_args!("{}: {},", rust_ident(&declaration.declarator.name), decode),
            );
        }
        line(indent + 8, &mut self.out, format_args!("}})"));
        line(indent + 4, &mut self.out, format_args!("}}"));
        line(indent, &mut self.out, format_args!("}}"));
        self.out.push('\n');
        Ok(())
    }

    fn emit_enum_xdr_impl(&mut self, name: &str, body: &EnumBody, indent: usize) {
        line(
            indent,
            &mut self.out,
            format_args!("impl XdrEncode for {} {{", name),
        );
        line(
            indent + 4,
            &mut self.out,
            format_args!(
                "fn encode_xdr(&self, output: &mut bytes::BytesMut) -> Result<(), onc_rpc_xdr::XdrError> {{"
            ),
        );
        line(
            indent + 8,
            &mut self.out,
            format_args!("(*self as i32).encode_xdr(output)"),
        );
        line(indent + 4, &mut self.out, format_args!("}}"));
        line(indent, &mut self.out, format_args!("}}"));
        self.out.push('\n');

        line(
            indent,
            &mut self.out,
            format_args!("impl XdrDecode for {} {{", name),
        );
        line(
            indent + 4,
            &mut self.out,
            format_args!(
                "fn decode_xdr(input: &mut &[u8]) -> Result<Self, onc_rpc_xdr::XdrError> {{"
            ),
        );
        line(
            indent + 8,
            &mut self.out,
            format_args!("match i32::decode_xdr(input)? {{"),
        );
        for variant in &body.variants {
            let value = self.render_value(&variant.value);
            line(
                indent + 12,
                &mut self.out,
                format_args!("{} => Ok(Self::{}),", value, variant.name),
            );
        }
        line(
            indent + 12,
            &mut self.out,
            format_args!("other => Err(onc_rpc_xdr::XdrError::InvalidEnum(other)),"),
        );
        line(indent + 8, &mut self.out, format_args!("}}"));
        line(indent + 4, &mut self.out, format_args!("}}"));
        line(indent, &mut self.out, format_args!("}}"));
        self.out.push('\n');
    }

    fn emit_union_xdr_impl(
        &mut self,
        name: &str,
        body: &UnionBody,
        discriminant_type: &str,
        indent: usize,
    ) -> Result<(), GeneratorError> {
        let decode_discriminant_type = self.union_decode_discriminant_type(body, discriminant_type);
        let decode_uses_raw_enum = decode_discriminant_type != discriminant_type;
        line(
            indent,
            &mut self.out,
            format_args!("impl XdrEncode for {} {{", name),
        );
        line(
            indent + 4,
            &mut self.out,
            format_args!(
                "fn encode_xdr(&self, output: &mut bytes::BytesMut) -> Result<(), onc_rpc_xdr::XdrError> {{"
            ),
        );
        line(indent + 8, &mut self.out, format_args!("match self {{"));
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
                let disc = self.render_union_label(label, &body.discriminant.type_spec);
                if matches!(label, UnionCaseLabel::Default) {
                    if rust_type == "()" {
                        line(
                            indent + 12,
                            &mut self.out,
                            format_args!("Self::{} {{ discriminant }} => {{", variant_name),
                        );
                        line(
                            indent + 16,
                            &mut self.out,
                            format_args!("discriminant.encode_xdr(output)?;"),
                        );
                    } else {
                        line(
                            indent + 12,
                            &mut self.out,
                            format_args!(
                                "Self::{} {{ discriminant, {} }} => {{",
                                variant_name,
                                rust_ident(&arm.declaration.declarator.name)
                            ),
                        );
                        line(
                            indent + 16,
                            &mut self.out,
                            format_args!("discriminant.encode_xdr(output)?;"),
                        );
                        self.emit_encode_declaration(
                            &arm.declaration.type_spec,
                            &arm.declaration.declarator,
                            rust_ident(&arm.declaration.declarator.name).as_str(),
                            indent + 16,
                        )?;
                    }
                } else if rust_type == "()" {
                    line(
                        indent + 12,
                        &mut self.out,
                        format_args!("Self::{} => {{", variant_name),
                    );
                    line(
                        indent + 16,
                        &mut self.out,
                        format_args!("({}).encode_xdr(output)?;", disc),
                    );
                } else {
                    line(
                        indent + 12,
                        &mut self.out,
                        format_args!(
                            "Self::{} {{ {} }} => {{",
                            variant_name,
                            rust_ident(&arm.declaration.declarator.name)
                        ),
                    );
                    line(
                        indent + 16,
                        &mut self.out,
                        format_args!("({}).encode_xdr(output)?;", disc),
                    );
                    self.emit_encode_declaration(
                        &arm.declaration.type_spec,
                        &arm.declaration.declarator,
                        rust_ident(&arm.declaration.declarator.name).as_str(),
                        indent + 16,
                    )?;
                }
                line(indent + 16, &mut self.out, format_args!("Ok(())"));
                line(indent + 12, &mut self.out, format_args!("}}"));
            }
        }
        line(indent + 8, &mut self.out, format_args!("}}"));
        line(indent + 4, &mut self.out, format_args!("}}"));
        line(indent, &mut self.out, format_args!("}}"));
        self.out.push('\n');

        line(
            indent,
            &mut self.out,
            format_args!("impl XdrDecode for {} {{", name),
        );
        line(
            indent + 4,
            &mut self.out,
            format_args!(
                "fn decode_xdr(input: &mut &[u8]) -> Result<Self, onc_rpc_xdr::XdrError> {{"
            ),
        );
        line(
            indent + 8,
            &mut self.out,
            format_args!(
                "let discriminant = <{} as XdrDecode>::decode_xdr(input)?;",
                decode_discriminant_type
            ),
        );
        line(
            indent + 8,
            &mut self.out,
            format_args!("match discriminant {{"),
        );
        for arm in &body.arms {
            let hint = format!("{}_{}", name, arm.declaration.declarator.name);
            let rust_type = self.rust_type_for_declaration(
                &arm.declaration.type_spec,
                &arm.declaration.declarator,
                &hint,
                indent,
            )?;
            let default = arm
                .labels
                .iter()
                .any(|label| matches!(label, UnionCaseLabel::Default));
            if default {
                continue;
            }
            for label in &arm.labels {
                let disc = self.render_union_decode_label(
                    label,
                    &body.discriminant.type_spec,
                    decode_uses_raw_enum,
                );
                let variant_name = union_variant_name(label);
                if rust_type == "()" {
                    line(
                        indent + 12,
                        &mut self.out,
                        format_args!("{} => Ok(Self::{}),", disc, variant_name),
                    );
                } else {
                    let decode = self.decode_declaration_expr(
                        &arm.declaration.type_spec,
                        &arm.declaration.declarator,
                        &hint,
                        indent + 12,
                    )?;
                    line(
                        indent + 12,
                        &mut self.out,
                        format_args!(
                            "{} => Ok(Self::{} {{ {}: {} }}),",
                            disc,
                            variant_name,
                            rust_ident(&arm.declaration.declarator.name),
                            decode
                        ),
                    );
                }
            }
        }
        if let Some(default_arm) = body.arms.iter().find(|arm| {
            arm.labels
                .iter()
                .any(|label| matches!(label, UnionCaseLabel::Default))
        }) {
            let hint = format!("{}_{}", name, default_arm.declaration.declarator.name);
            let rust_type = self.rust_type_for_declaration(
                &default_arm.declaration.type_spec,
                &default_arm.declaration.declarator,
                &hint,
                indent,
            )?;
            if rust_type == "()" {
                line(
                    indent + 12,
                    &mut self.out,
                    format_args!("discriminant => Ok(Self::Default {{ discriminant }}),"),
                );
            } else {
                let decode = self.decode_declaration_expr(
                    &default_arm.declaration.type_spec,
                    &default_arm.declaration.declarator,
                    &hint,
                    indent + 12,
                )?;
                line(
                    indent + 12,
                    &mut self.out,
                    format_args!(
                        "discriminant => Ok(Self::Default {{ discriminant, {}: {} }}),",
                        rust_ident(&default_arm.declaration.declarator.name),
                        decode
                    ),
                );
            }
        } else {
            line(
                indent + 12,
                &mut self.out,
                format_args!("_ => Err(onc_rpc_xdr::XdrError::InvalidEnum(0)),"),
            );
        }
        line(indent + 8, &mut self.out, format_args!("}}"));
        line(indent + 4, &mut self.out, format_args!("}}"));
        line(indent, &mut self.out, format_args!("}}"));
        self.out.push('\n');
        Ok(())
    }

    fn emit_encode_declaration(
        &mut self,
        target: &TypeSpec,
        declarator: &Declarator,
        access: &str,
        indent: usize,
    ) -> Result<(), GeneratorError> {
        match &declarator.modifier {
            Some(DeclaratorModifier::VariableArray(_)) => match target {
                TypeSpec::Opaque | TypeSpec::String => {
                    line(
                        indent,
                        &mut self.out,
                        format_args!("{access}.encode_xdr(output)?;"),
                    );
                }
                _ => {
                    line(
                        indent,
                        &mut self.out,
                        format_args!("({access}.len() as u32).encode_xdr(output)?;"),
                    );
                    line(
                        indent,
                        &mut self.out,
                        format_args!("for value in &{access} {{"),
                    );
                    line(
                        indent + 4,
                        &mut self.out,
                        format_args!("value.encode_xdr(output)?;"),
                    );
                    line(indent, &mut self.out, format_args!("}}"));
                }
            },
            Some(DeclaratorModifier::FixedArray(_)) => match target {
                TypeSpec::Opaque => {
                    line(
                        indent,
                        &mut self.out,
                        format_args!("onc_rpc_xdr::write_fixed_opaque(output, &{access});"),
                    );
                }
                _ => {
                    line(
                        indent,
                        &mut self.out,
                        format_args!("for value in &{access} {{"),
                    );
                    line(
                        indent + 4,
                        &mut self.out,
                        format_args!("value.encode_xdr(output)?;"),
                    );
                    line(indent, &mut self.out, format_args!("}}"));
                }
            },
            Some(DeclaratorModifier::Optional) | None => {
                line(
                    indent,
                    &mut self.out,
                    format_args!("{access}.encode_xdr(output)?;"),
                );
            }
        }
        Ok(())
    }

    fn decode_declaration_expr(
        &mut self,
        target: &TypeSpec,
        declarator: &Declarator,
        hint: &str,
        indent: usize,
    ) -> Result<String, GeneratorError> {
        let base = self.rust_type_for_base(target, hint, indent)?;
        match &declarator.modifier {
            Some(DeclaratorModifier::VariableArray(_)) => match target {
                TypeSpec::Opaque => Ok("bytes::Bytes::decode_xdr(input)?".to_string()),
                TypeSpec::String => Ok("String::decode_xdr(input)?".to_string()),
                _ => Ok(format!("Vec::<{}>::decode_xdr(input)?", base)),
            },
            Some(DeclaratorModifier::FixedArray(bound)) => match target {
                TypeSpec::Opaque => Ok(format!(
                    "<[u8; {}] as XdrDecode>::decode_xdr(input)?",
                    self.render_array_bound(bound)?
                )),
                _ => Ok(format!(
                    "onc_rpc_xdr::decode_fixed_array::<{}, {}>(input)?",
                    base,
                    self.render_array_bound(bound)?
                )),
            },
            Some(DeclaratorModifier::Optional) => {
                Ok(format!("Option::<Box<{}>>::decode_xdr(input)?", base))
            }
            None => Ok(format!("<{} as XdrDecode>::decode_xdr(input)?", base)),
        }
    }

    fn render_union_label(&self, label: &UnionCaseLabel, discriminant_type: &TypeSpec) -> String {
        match label {
            UnionCaseLabel::Case(ValueExpr::Identifier(name))
                if matches!(discriminant_type, TypeSpec::Bool) && name == "TRUE" =>
            {
                "true".to_string()
            }
            UnionCaseLabel::Case(ValueExpr::Identifier(name))
                if matches!(discriminant_type, TypeSpec::Bool) && name == "FALSE" =>
            {
                "false".to_string()
            }
            UnionCaseLabel::Case(ValueExpr::Identifier(name)) => {
                if let Some(enum_name) = self.enum_type_name_for(discriminant_type) {
                    format!(
                        "{}::{}",
                        self.qualify_type_name(enum_name),
                        rust_ident(name)
                    )
                } else {
                    self.render_value(&ValueExpr::Identifier(name.clone()))
                }
            }
            UnionCaseLabel::Case(value) => self.render_value(value),
            UnionCaseLabel::Default => "discriminant".to_string(),
        }
    }

    fn render_union_decode_label(
        &self,
        label: &UnionCaseLabel,
        discriminant_type: &TypeSpec,
        raw_enum_discriminant: bool,
    ) -> String {
        if !raw_enum_discriminant {
            return self.render_union_label(label, discriminant_type);
        }

        match label {
            UnionCaseLabel::Case(ValueExpr::Identifier(name)) => {
                let Some(enum_name) = self.enum_type_name_for(discriminant_type) else {
                    return self.render_value(&ValueExpr::Identifier(name.clone()));
                };
                format!(
                    "value if value == {}::{} as i32",
                    self.qualify_type_name(enum_name),
                    rust_ident(name)
                )
            }
            UnionCaseLabel::Case(value) => self.render_value(value),
            UnionCaseLabel::Default => "discriminant".to_string(),
        }
    }

    fn union_decode_discriminant_type<'a>(
        &self,
        body: &UnionBody,
        discriminant_type: &'a str,
    ) -> &'a str {
        if self.union_has_default_arm(body)
            && self
                .enum_type_name_for(&body.discriminant.type_spec)
                .is_some()
        {
            "i32"
        } else {
            discriminant_type
        }
    }

    fn union_default_discriminant_type<'a>(
        &self,
        discriminant_type_spec: &TypeSpec,
        discriminant_type: &'a str,
    ) -> &'a str {
        if self.enum_type_name_for(discriminant_type_spec).is_some() {
            "i32"
        } else {
            discriminant_type
        }
    }

    fn union_has_default_arm(&self, body: &UnionBody) -> bool {
        body.arms.iter().any(|arm| {
            arm.labels
                .iter()
                .any(|label| matches!(label, UnionCaseLabel::Default))
        })
    }

    fn enum_type_name_for<'a>(&'a self, discriminant_type: &'a TypeSpec) -> Option<&'a str> {
        match discriminant_type {
            TypeSpec::Identifier(name) => match self.named_types.get(name) {
                Some(NamedType::Enum) => Some(name.as_str()),
                _ => None,
            },
            _ => None,
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

fn schema_uses_bytes(schema: &Schema) -> bool {
    schema.items.iter().any(item_uses_bytes)
}

fn schema_uses_xdr_traits(schema: &Schema) -> bool {
    schema.items.iter().any(item_uses_xdr_traits)
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

fn item_uses_xdr_traits(item: &Item) -> bool {
    match item {
        Item::Struct(_) | Item::Enum(_) | Item::Union(_) => true,
        Item::Typedef(item) => type_spec_uses_xdr_traits(&item.target),
        Item::Const(_) | Item::Program(_) => false,
    }
}

fn type_spec_uses_xdr_traits(target: &TypeSpec) -> bool {
    matches!(
        target,
        TypeSpec::Enum(_) | TypeSpec::Struct(_) | TypeSpec::Union(_)
    )
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

fn rust_ident(input: &str) -> String {
    if is_rust_keyword(input) {
        format!("r#{input}")
    } else {
        input.to_string()
    }
}

fn is_rust_keyword(input: &str) -> bool {
    matches!(
        input,
        "as" | "break"
            | "const"
            | "continue"
            | "crate"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "async"
            | "await"
            | "dyn"
    )
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
