use std::fmt::Write;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schema {
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Const(ConstDecl),
    Typedef(TypedefDecl),
    Struct(StructDecl),
    Enum(EnumDecl),
    Union(UnionDecl),
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
    pub body: StructBody,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumDecl {
    pub name: String,
    pub body: EnumBody,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnionDecl {
    pub name: String,
    pub body: UnionBody,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructBody {
    pub declarations: Vec<Declaration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumBody {
    pub variants: Vec<EnumVariant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnionBody {
    pub discriminant: Declaration,
    pub arms: Vec<UnionArm>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnionArm {
    pub labels: Vec<UnionCaseLabel>,
    pub declaration: Declaration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnionCaseLabel {
    Case(ValueExpr),
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration {
    pub type_spec: TypeSpec,
    pub declarator: Declarator,
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
pub struct Declarator {
    pub name: String,
    pub modifier: Option<DeclaratorModifier>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclaratorModifier {
    FixedArray(ValueExpr),
    VariableArray(Option<ValueExpr>),
    Optional,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeSpec {
    Void,
    Bool,
    Int,
    UnsignedInt,
    Hyper,
    UnsignedHyper,
    Float,
    Double,
    Quadruple,
    Opaque,
    String,
    Identifier(String),
    Enum(EnumBody),
    Struct(Box<StructBody>),
    Union(Box<UnionBody>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueExpr {
    Number(i64),
    Identifier(String),
}

impl Schema {
    pub fn render_snapshot(&self) -> String {
        let mut out = String::new();

        for item in &self.items {
            match item {
                Item::Const(item) => {
                    let _ = writeln!(out, "const {} = {}", item.name, render_value(&item.value));
                }
                Item::Typedef(item) => {
                    let _ = writeln!(
                        out,
                        "typedef {} {}",
                        render_type_spec(&item.target),
                        render_declarator(&item.declarator)
                    );
                }
                Item::Struct(item) => {
                    let _ = writeln!(out, "struct {}", item.name);
                    render_struct_body(&item.body, 2, &mut out);
                }
                Item::Enum(item) => {
                    let _ = writeln!(out, "enum {}", item.name);
                    render_enum_body(&item.body, 2, &mut out);
                }
                Item::Union(item) => {
                    let _ = writeln!(out, "union {}", item.name);
                    render_union_body(&item.body, 2, &mut out);
                }
                Item::Program(item) => {
                    let _ = writeln!(out, "program {} = {}", item.name, item.number);
                    for version in &item.versions {
                        let _ = writeln!(out, "  version {} = {}", version.name, version.number);
                        for procedure in &version.procedures {
                            let _ = writeln!(
                                out,
                                "    {} {}({}) = {}",
                                render_type_spec(&procedure.return_type),
                                procedure.name,
                                render_type_spec(&procedure.argument_type),
                                procedure.number
                            );
                        }
                    }
                }
            }
        }

        out
    }
}

fn render_struct_body(body: &StructBody, indent: usize, out: &mut String) {
    for declaration in &body.declarations {
        let _ = writeln!(
            out,
            "{:indent$}{} {}",
            "",
            render_type_spec(&declaration.type_spec),
            render_declarator(&declaration.declarator),
            indent = indent
        );
    }
}

fn render_enum_body(body: &EnumBody, indent: usize, out: &mut String) {
    for variant in &body.variants {
        let _ = writeln!(
            out,
            "{:indent$}{} = {}",
            "",
            variant.name,
            render_value(&variant.value),
            indent = indent
        );
    }
}

fn render_union_body(body: &UnionBody, indent: usize, out: &mut String) {
    let _ = writeln!(
        out,
        "{:indent$}switch {} {}",
        "",
        render_type_spec(&body.discriminant.type_spec),
        render_declarator(&body.discriminant.declarator),
        indent = indent
    );
    for arm in &body.arms {
        for label in &arm.labels {
            match label {
                UnionCaseLabel::Case(value) => {
                    let _ = writeln!(
                        out,
                        "{:indent$}case {}",
                        "",
                        render_value(value),
                        indent = indent + 2
                    );
                }
                UnionCaseLabel::Default => {
                    let _ = writeln!(out, "{:indent$}default", "", indent = indent + 2);
                }
            }
        }
        let _ = writeln!(
            out,
            "{:indent$}{} {}",
            "",
            render_type_spec(&arm.declaration.type_spec),
            render_declarator(&arm.declaration.declarator),
            indent = indent + 4
        );
    }
}

fn render_type_spec(type_spec: &TypeSpec) -> String {
    match type_spec {
        TypeSpec::Void => "void".to_string(),
        TypeSpec::Bool => "bool".to_string(),
        TypeSpec::Int => "int".to_string(),
        TypeSpec::UnsignedInt => "unsigned int".to_string(),
        TypeSpec::Hyper => "hyper".to_string(),
        TypeSpec::UnsignedHyper => "unsigned hyper".to_string(),
        TypeSpec::Float => "float".to_string(),
        TypeSpec::Double => "double".to_string(),
        TypeSpec::Quadruple => "quadruple".to_string(),
        TypeSpec::Opaque => "opaque".to_string(),
        TypeSpec::String => "string".to_string(),
        TypeSpec::Identifier(name) => name.clone(),
        TypeSpec::Enum(body) => render_inline_enum(body),
        TypeSpec::Struct(body) => render_inline_struct(body),
        TypeSpec::Union(body) => render_inline_union(body),
    }
}

fn render_inline_enum(body: &EnumBody) -> String {
    let mut out = String::from("enum { ");
    for (idx, variant) in body.variants.iter().enumerate() {
        if idx > 0 {
            out.push_str(", ");
        }
        let _ = write!(out, "{} = {}", variant.name, render_value(&variant.value));
    }
    out.push_str(" }");
    out
}

fn render_inline_struct(body: &StructBody) -> String {
    let mut out = String::from("struct { ");
    for (idx, declaration) in body.declarations.iter().enumerate() {
        if idx > 0 {
            out.push_str("; ");
        }
        let _ = write!(
            out,
            "{} {}",
            render_type_spec(&declaration.type_spec),
            render_declarator(&declaration.declarator)
        );
    }
    if !body.declarations.is_empty() {
        out.push(';');
    }
    out.push_str(" }");
    out
}

fn render_inline_union(body: &UnionBody) -> String {
    let mut out = String::new();
    let _ = write!(
        out,
        "union switch ({} {}) {{ ",
        render_type_spec(&body.discriminant.type_spec),
        render_declarator(&body.discriminant.declarator)
    );
    for (arm_idx, arm) in body.arms.iter().enumerate() {
        if arm_idx > 0 {
            out.push(' ');
        }
        for label in &arm.labels {
            match label {
                UnionCaseLabel::Case(value) => {
                    let _ = write!(out, "case {}: ", render_value(value));
                }
                UnionCaseLabel::Default => out.push_str("default: "),
            }
        }
        let _ = write!(
            out,
            "{} {}; ",
            render_type_spec(&arm.declaration.type_spec),
            render_declarator(&arm.declaration.declarator)
        );
    }
    out.push('}');
    out
}

fn render_declarator(declarator: &Declarator) -> String {
    let mut out = String::new();
    if matches!(declarator.modifier, Some(DeclaratorModifier::Optional)) {
        out.push('*');
    }
    out.push_str(&declarator.name);
    match &declarator.modifier {
        Some(DeclaratorModifier::FixedArray(bound)) => {
            let _ = write!(out, "[{}]", render_value(bound));
        }
        Some(DeclaratorModifier::VariableArray(Some(bound))) => {
            let _ = write!(out, "<{}>", render_value(bound));
        }
        Some(DeclaratorModifier::VariableArray(None)) => out.push_str("<>"),
        Some(DeclaratorModifier::Optional) | None => {}
    }
    out
}

fn render_value(value: &ValueExpr) -> String {
    match value {
        ValueExpr::Number(value) => value.to_string(),
        ValueExpr::Identifier(name) => name.clone(),
    }
}
