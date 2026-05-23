use std::fmt::Write;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schema {
    pub items: Vec<Item>,
}

impl Schema {
    pub fn render_snapshot(&self) -> String {
        let mut out = String::new();

        for item in &self.items {
            render_item(item, 0, &mut out);
        }

        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Const(ConstDecl),
    Typedef(TypedefDecl),
    Struct(StructDecl),
    Enum(EnumDecl),
    Program(ProgramDecl),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstDecl {
    pub name: String,
    pub value: ValueExpr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedefDecl {
    pub target: TypeSpec,
    pub declarator: Declarator,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructDecl {
    pub name: String,
    pub fields: Vec<FieldDecl>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumDecl {
    pub name: String,
    pub variants: Vec<EnumVariant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumVariant {
    pub name: String,
    pub value: ValueExpr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramDecl {
    pub name: String,
    pub versions: Vec<VersionDecl>,
    pub number: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionDecl {
    pub name: String,
    pub procedures: Vec<ProcedureDecl>,
    pub number: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcedureDecl {
    pub return_type: TypeSpec,
    pub name: String,
    pub argument_type: TypeSpec,
    pub number: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDecl {
    pub field_type: TypeSpec,
    pub declarator: Declarator,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declarator {
    pub name: String,
    pub modifier: Option<DeclaratorModifier>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclaratorModifier {
    VariableArray(Option<ValueExpr>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeSpec {
    Void,
    Bool,
    Int,
    UnsignedInt,
    Hyper,
    UnsignedHyper,
    Opaque,
    String,
    Identifier(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueExpr {
    Number(i64),
    Identifier(String),
}

fn render_item(item: &Item, indent: usize, out: &mut String) {
    match item {
        Item::Const(item) => {
            line(
                indent,
                out,
                format_args!("const {} = {}", item.name, render_value(&item.value)),
            );
        }
        Item::Typedef(item) => {
            line(
                indent,
                out,
                format_args!(
                    "typedef {} {}",
                    render_type(&item.target),
                    render_declarator(&item.declarator)
                ),
            );
        }
        Item::Struct(item) => {
            line(indent, out, format_args!("struct {}", item.name));
            for field in &item.fields {
                line(
                    indent + 2,
                    out,
                    format_args!(
                        "{} {}",
                        render_type(&field.field_type),
                        render_declarator(&field.declarator)
                    ),
                );
            }
        }
        Item::Enum(item) => {
            line(indent, out, format_args!("enum {}", item.name));
            for variant in &item.variants {
                line(
                    indent + 2,
                    out,
                    format_args!("{} = {}", variant.name, render_value(&variant.value)),
                );
            }
        }
        Item::Program(item) => {
            line(
                indent,
                out,
                format_args!("program {} = {}", item.name, item.number),
            );
            for version in &item.versions {
                line(
                    indent + 2,
                    out,
                    format_args!("version {} = {}", version.name, version.number),
                );
                for procedure in &version.procedures {
                    line(
                        indent + 4,
                        out,
                        format_args!(
                            "{} {}({}) = {}",
                            render_type(&procedure.return_type),
                            procedure.name,
                            render_type(&procedure.argument_type),
                            procedure.number
                        ),
                    );
                }
            }
        }
    }
}

fn render_type(ty: &TypeSpec) -> String {
    match ty {
        TypeSpec::Void => "void".to_string(),
        TypeSpec::Bool => "bool".to_string(),
        TypeSpec::Int => "int".to_string(),
        TypeSpec::UnsignedInt => "unsigned int".to_string(),
        TypeSpec::Hyper => "hyper".to_string(),
        TypeSpec::UnsignedHyper => "unsigned hyper".to_string(),
        TypeSpec::Opaque => "opaque".to_string(),
        TypeSpec::String => "string".to_string(),
        TypeSpec::Identifier(name) => name.clone(),
    }
}

fn render_declarator(declarator: &Declarator) -> String {
    match &declarator.modifier {
        None => declarator.name.clone(),
        Some(DeclaratorModifier::VariableArray(bound)) => match bound {
            Some(bound) => format!("{}<{}>", declarator.name, render_value(bound)),
            None => format!("{}<>", declarator.name),
        },
    }
}

fn render_value(value: &ValueExpr) -> String {
    match value {
        ValueExpr::Number(value) => value.to_string(),
        ValueExpr::Identifier(name) => name.clone(),
    }
}

fn line(indent: usize, out: &mut String, args: std::fmt::Arguments<'_>) {
    let _ = write!(out, "{:indent$}", "", indent = indent);
    let _ = out.write_fmt(args);
    out.push('\n');
}
