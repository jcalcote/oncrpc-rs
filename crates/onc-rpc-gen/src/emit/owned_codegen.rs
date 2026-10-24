//! Additive caller-owned XDR generation; ordinary payload types stay unchanged.
use super::*;

#[derive(Clone, Copy)]
struct Shape {
    prefix: usize,
    body: bool,
}

pub(super) struct OwnedLayouts {
    items: BTreeMap<String, Item>,
    constants: BTreeMap<String, ValueExpr>,
}
impl OwnedLayouts {
    pub(super) fn from_schema(schema: &Schema) -> Self {
        Self::from_modules(std::slice::from_ref(schema))
    }

    pub(super) fn from_modules(schemas: &[Schema]) -> Self {
        let mut result = Self {
            items: BTreeMap::new(),
            constants: BTreeMap::new(),
        };
        for schema in schemas {
            for item in &schema.items {
                let name = match item {
                    Item::Typedef(item) => &item.declarator.name,
                    Item::Struct(item) => &item.name,
                    Item::Union(item) => &item.name,
                    Item::Enum(item) => &item.name,
                    Item::Const(item) => {
                        result
                            .constants
                            .insert(item.name.clone(), item.value.clone());
                        continue;
                    }
                    Item::Program(_) => continue,
                };
                result.items.insert(name.clone(), item.clone());
            }
        }
        result
    }

    fn value(&self, value: &ValueExpr, seen: &mut HashSet<String>) -> Option<usize> {
        match value {
            ValueExpr::Number(number) => usize::try_from(*number).ok(),
            ValueExpr::Identifier(name) => {
                if !seen.insert(name.clone()) {
                    return None;
                }
                let value = self.value(self.constants.get(name)?, seen);
                seen.remove(name);
                value
            }
        }
    }

    fn named(&self, name: &str, seen: &mut HashSet<String>) -> Option<Shape> {
        if !seen.insert(name.to_string()) {
            return None;
        }
        let result = match self.items.get(name)? {
            Item::Typedef(item) => self.declaration(&item.target, &item.declarator.modifier, seen),
            Item::Struct(item) => self.structure(&item.body, seen),
            Item::Enum(_) => Some(Shape {
                prefix: 4,
                body: false,
            }),
            Item::Union(item) => self.union(&item.body, seen),
            _ => None,
        };
        seen.remove(name);
        result
    }

    fn base(&self, target: &TypeSpec, seen: &mut HashSet<String>) -> Option<Shape> {
        Some(match target {
            TypeSpec::Void => Shape {
                prefix: 0,
                body: false,
            },
            TypeSpec::Bool
            | TypeSpec::Int
            | TypeSpec::UnsignedInt
            | TypeSpec::Float
            | TypeSpec::Enum(_) => Shape {
                prefix: 4,
                body: false,
            },
            TypeSpec::Hyper | TypeSpec::UnsignedHyper | TypeSpec::Double => Shape {
                prefix: 8,
                body: false,
            },
            TypeSpec::Quadruple => Shape {
                prefix: 16,
                body: false,
            },
            TypeSpec::Opaque => Shape {
                prefix: 1,
                body: false,
            },
            TypeSpec::Identifier(name) => self.named(name, seen)?,
            TypeSpec::Struct(body) => {
                let shape = self.structure(body, seen)?;
                if shape.body {
                    return None;
                } // Inline body projections need a named IDL type.
                shape
            }
            TypeSpec::Union(body) => {
                let shape = self.union(body, seen)?;
                if shape.body {
                    return None;
                }
                shape
            }
            TypeSpec::String => return None,
        })
    }

    fn declaration(
        &self,
        target: &TypeSpec,
        modifier: &Option<DeclaratorModifier>,
        seen: &mut HashSet<String>,
    ) -> Option<Shape> {
        match modifier {
            Some(DeclaratorModifier::VariableArray(bound))
                if matches!(target, TypeSpec::Opaque) =>
            {
                if let Some(bound) = bound {
                    self.value(bound, &mut HashSet::new())?;
                }
                Some(Shape {
                    prefix: 4,
                    body: true,
                })
            }
            Some(DeclaratorModifier::VariableArray(_)) => None,
            Some(DeclaratorModifier::FixedArray(bound)) => {
                let count = self.value(bound, &mut HashSet::new())?;
                let base = self.base(target, seen)?;
                if base.body {
                    return None;
                }
                let prefix = if matches!(target, TypeSpec::Opaque) {
                    count.checked_add(3)? / 4 * 4
                } else {
                    count.checked_mul(base.prefix)?
                };
                Some(Shape {
                    prefix,
                    body: false,
                })
            }
            Some(DeclaratorModifier::Optional) => {
                let base = self.base(target, seen)?;
                Some(Shape {
                    prefix: base.prefix.checked_add(4)?,
                    body: base.body,
                })
            }
            None => self.base(target, seen),
        }
    }

    fn structure(&self, body: &StructBody, seen: &mut HashSet<String>) -> Option<Shape> {
        let mut prefix = 0usize;
        let mut has_body = false;
        for (index, declaration) in body.declarations.iter().enumerate() {
            let shape = self.declaration(
                &declaration.type_spec,
                &declaration.declarator.modifier,
                seen,
            )?;
            if shape.body && index + 1 != body.declarations.len() {
                return None;
            }
            prefix = prefix.checked_add(shape.prefix)?;
            has_body |= shape.body;
        }
        Some(Shape {
            prefix,
            body: has_body,
        })
    }

    fn union(&self, body: &UnionBody, seen: &mut HashSet<String>) -> Option<Shape> {
        let discriminant = self.declaration(
            &body.discriminant.type_spec,
            &body.discriminant.declarator.modifier,
            seen,
        )?;
        if discriminant.body {
            return None;
        }
        let mut max_arm = 0;
        let mut has_body = false;
        for arm in &body.arms {
            let shape = self.declaration(
                &arm.declaration.type_spec,
                &arm.declaration.declarator.modifier,
                seen,
            )?;
            max_arm = max_arm.max(shape.prefix);
            has_body |= shape.body;
        }
        Some(Shape {
            prefix: discriminant.prefix.checked_add(max_arm)?,
            body: has_body,
        })
    }

    pub(super) fn metadata_name(&self, name: &str) -> Option<String> {
        self.named(name, &mut HashSet::new())
            .filter(|shape| shape.body)
            .map(|_| format!("{name}Metadata"))
    }

    fn terminal(
        &self,
        target: &TypeSpec,
        modifier: &Option<DeclaratorModifier>,
        seen: &mut HashSet<String>,
    ) -> bool {
        match (target, modifier) {
            (TypeSpec::Opaque, Some(DeclaratorModifier::VariableArray(_))) => true,
            (TypeSpec::Identifier(name), None | Some(DeclaratorModifier::Optional)) => {
                if !seen.insert(name.clone()) {
                    return false;
                }
                let terminal = match self.items.get(name) {
                    Some(Item::Typedef(item)) => {
                        self.terminal(&item.target, &item.declarator.modifier, seen)
                    }
                    Some(Item::Struct(item)) => {
                        item.body.declarations.last().is_some_and(|declaration| {
                            self.terminal(
                                &declaration.type_spec,
                                &declaration.declarator.modifier,
                                seen,
                            )
                        })
                    }
                    Some(Item::Union(item)) => item.body.arms.iter().any(|arm| {
                        self.terminal(
                            &arm.declaration.type_spec,
                            &arm.declaration.declarator.modifier,
                            seen,
                        )
                    }),
                    _ => false,
                };
                seen.remove(name);
                terminal
            }
            _ => false,
        }
    }

    fn segments_name(&self, name: &str) -> Option<String> {
        self.terminal(
            &TypeSpec::Identifier(name.to_string()),
            &None,
            &mut HashSet::new(),
        )
        .then(|| format!("{name}Segments"))
    }

    fn transform_segments(&self, declaration: &mut Declaration, emitter: &TypeEmitter) {
        match (&declaration.type_spec, &declaration.declarator.modifier) {
            (TypeSpec::Opaque, Some(DeclaratorModifier::VariableArray(bound))) => {
                let maximum = bound
                    .as_ref()
                    .map(|value| emitter.render_value(value))
                    .unwrap_or_else(|| "u32::MAX".into());
                declaration.type_spec = TypeSpec::Identifier(format!(
                    "onc_rpc_xdr::OpaqueSegments<{{ {maximum} as usize }}>"
                ));
                declaration.declarator.modifier = None;
            }
            (TypeSpec::Identifier(name), _) => {
                if let Some(segments) = self.segments_name(name) {
                    declaration.type_spec = TypeSpec::Identifier(segments);
                }
            }
            _ => {}
        }
    }

    fn conversion(&self, declaration: &Declaration, access: &str) -> String {
        self.convert_value(
            &declaration.type_spec,
            &declaration.declarator.modifier,
            access,
            &mut HashSet::new(),
        )
    }

    fn convert_value(
        &self,
        target: &TypeSpec,
        modifier: &Option<DeclaratorModifier>,
        access: &str,
        seen: &mut HashSet<String>,
    ) -> String {
        if matches!(modifier, Some(DeclaratorModifier::Optional)) {
            let inner = self.convert_value(target, &None, "(*value)", seen);
            return format!("{access}.map(|value| Box::new({inner}))");
        }
        if let (TypeSpec::Identifier(name), None) = (target, modifier) {
            if seen.insert(name.clone()) {
                if let Some(Item::Typedef(item)) = self.items.get(name) {
                    return self.convert_value(
                        &item.target,
                        &item.declarator.modifier,
                        access,
                        seen,
                    );
                }
            }
        }
        format!("{access}.into()")
    }

    fn transform(&self, declaration: &mut Declaration, emitter: &TypeEmitter) {
        match (&declaration.type_spec, &declaration.declarator.modifier) {
            (TypeSpec::Opaque, Some(DeclaratorModifier::VariableArray(bound))) => {
                let maximum = bound
                    .as_ref()
                    .map(|value| emitter.render_value(value))
                    .unwrap_or_else(|| "u32::MAX".into());
                declaration.type_spec = TypeSpec::Identifier(format!(
                    "onc_rpc_xdr::OpaqueLength<{{ {maximum} as usize }}>"
                ));
                declaration.declarator.modifier = None;
            }
            (TypeSpec::Identifier(name), _) => {
                if let Some(metadata) = self.metadata_name(name) {
                    declaration.type_spec = TypeSpec::Identifier(metadata);
                }
            }
            _ => {}
        }
    }

    fn opaque_expr(&self, declaration: &Declaration, access: &str) -> String {
        match &declaration.declarator.modifier {
            Some(DeclaratorModifier::Optional) => {
                format!("({access}).as_deref().and_then(onc_rpc_xdr::XdrReplyMetadata::opaque_len)")
            }
            _ => format!(
                "onc_rpc_xdr::XdrReplyMetadata::opaque_len({}{access})",
                if access.starts_with("self.") { "&" } else { "" }
            ),
        }
    }
}

impl TypeEmitter {
    pub(super) fn emit_metadata(
        &mut self,
        schema: &Schema,
        layouts: &OwnedLayouts,
    ) -> Result<(), GeneratorError> {
        for (name, owner) in self.type_owners.clone() {
            if let Some(metadata) = layouts.metadata_name(&name) {
                if layouts.items.contains_key(&metadata) {
                    return Err(GeneratorError::UnsupportedConstruct(format!(
                        "generated metadata name collides with IDL type: {metadata}"
                    )));
                }
                if let Some(original) = self.named_types.get(&name).cloned() {
                    self.named_types.insert(metadata.clone(), original);
                }
                self.type_owners.insert(metadata, owner);
            }
        }
        for (name, owner) in self.type_owners.clone() {
            if let Some(segments) = layouts.segments_name(&name) {
                if layouts.items.contains_key(&segments) {
                    return Err(GeneratorError::UnsupportedConstruct(format!(
                        "generated segments name collides with IDL type: {segments}"
                    )));
                }
                if let Some(original) = self.named_types.get(&name).cloned() {
                    if !matches!(
                        original,
                        NamedType::Typedef {
                            target: TypeSpec::Opaque,
                            ..
                        }
                    ) {
                        self.named_types.insert(segments.clone(), original);
                    }
                }
                self.segment_projection_names.insert(segments.clone());
                self.type_owners.insert(segments, owner);
            }
        }
        for name in layouts.items.keys() {
            if let Some(original) = self.named_types.get(name).cloned() {
                if let Some(metadata) = layouts.metadata_name(name) {
                    self.named_types.insert(metadata, original.clone());
                }
                if let Some(segments) = layouts.segments_name(name) {
                    self.segment_projection_names.insert(segments.clone());
                    if !matches!(
                        original,
                        NamedType::Typedef {
                            target: TypeSpec::Opaque,
                            ..
                        }
                    ) {
                        self.named_types.insert(segments, original);
                    }
                }
            }
        }
        self.metadata_only = true;
        for item in &schema.items {
            let name = match item {
                Item::Typedef(item) => &item.declarator.name,
                Item::Struct(item) => &item.name,
                Item::Union(item) => &item.name,
                _ => continue,
            };
            let Some(metadata) = layouts.metadata_name(name) else {
                continue;
            };
            if layouts.items.contains_key(&metadata) {
                return Err(GeneratorError::UnsupportedConstruct(format!(
                    "generated metadata name collides with IDL type: {metadata}"
                )));
            }
            let shape = layouts
                .named(name, &mut HashSet::new())
                .expect("supported metadata");
            line(
                0,
                &mut self.out,
                format_args!(
                    "/// Metadata-only projection; the terminal opaque body remains in caller storage."
                ),
            );
            match item {
                Item::Typedef(item) => {
                    let mut declaration = Declaration {
                        type_spec: item.target.clone(),
                        declarator: item.declarator.clone(),
                    };
                    layouts.transform(&mut declaration, self);
                    let projected = TypedefDecl {
                        target: declaration.type_spec,
                        declarator: Declarator {
                            name: metadata,
                            modifier: declaration.declarator.modifier,
                        },
                    };
                    self.emit_typedef(&projected, 0)?;
                }
                Item::Struct(item) => {
                    let mut body = item.body.clone();
                    let last = body.declarations.last_mut().expect("body-bearing struct");
                    let access = format!("self.{}", rust_ident(&last.declarator.name));
                    let expression = layouts.opaque_expr(last, &access);
                    layouts.transform(last, self);
                    self.emit_named_struct(&metadata, &body, 0)?;
                    self.emit_metadata_trait(&metadata, shape.prefix, &expression);
                }
                Item::Union(item) => {
                    let mut body = item.body.clone();
                    let mut expressions = Vec::new();
                    for arm in &mut body.arms {
                        let has_body = layouts
                            .declaration(
                                &arm.declaration.type_spec,
                                &arm.declaration.declarator.modifier,
                                &mut HashSet::new(),
                            )
                            .is_some_and(|shape| shape.body);
                        let field = rust_ident(&arm.declaration.declarator.name);
                        let is_void = matches!(arm.declaration.type_spec, TypeSpec::Void);
                        for label in &arm.labels {
                            let variant = union_variant_name(label);
                            let pattern = if matches!(label, UnionCaseLabel::Default) {
                                format!(
                                    "Self::{variant} {{ {}.. }}",
                                    if has_body {
                                        format!("{field}, ")
                                    } else {
                                        String::new()
                                    }
                                )
                            } else if is_void {
                                format!("Self::{variant}")
                            } else if has_body {
                                format!("Self::{variant} {{ {field} }}")
                            } else {
                                format!("Self::{variant} {{ .. }}")
                            };
                            let expression = if has_body {
                                layouts.opaque_expr(&arm.declaration, &field)
                            } else {
                                "None".into()
                            };
                            expressions.push(format!("{pattern} => {expression},"));
                        }
                        if has_body {
                            layouts.transform(&mut arm.declaration, self);
                        }
                    }
                    self.emit_named_union(&metadata, &body, 0)?;
                    self.emit_metadata_trait(
                        &metadata,
                        shape.prefix,
                        &format!("match self {{ {} }}", expressions.join(" ")),
                    );
                }
                _ => unreachable!(),
            }
        }
        self.metadata_only = false;
        self.emit_segment_projections(schema, layouts)?;
        Ok(())
    }

    fn emit_segment_projections(
        &mut self,
        schema: &Schema,
        layouts: &OwnedLayouts,
    ) -> Result<(), GeneratorError> {
        for item in &schema.items {
            let name = match item {
                Item::Typedef(item) => &item.declarator.name,
                Item::Struct(item) => &item.name,
                Item::Union(item) => &item.name,
                _ => continue,
            };
            let Some(segments) = layouts.segments_name(name) else {
                continue;
            };
            if layouts.items.contains_key(&segments) {
                return Err(GeneratorError::UnsupportedConstruct(format!(
                    "generated segments name collides with IDL type: {segments}"
                )));
            }
            line(
                0,
                &mut self.out,
                format_args!(
                    "/// Alternate argument representation retaining multiple owners for one terminal opaque body."
                ),
            );
            match item {
                Item::Typedef(item) => {
                    let mut declaration = Declaration {
                        type_spec: item.target.clone(),
                        declarator: item.declarator.clone(),
                    };
                    layouts.transform_segments(&mut declaration, self);
                    self.emit_typedef(
                        &TypedefDecl {
                            target: declaration.type_spec,
                            declarator: Declarator {
                                name: segments,
                                modifier: declaration.declarator.modifier,
                            },
                        },
                        0,
                    )?;
                }
                Item::Struct(item) => {
                    let mut body = item.body.clone();
                    layouts.transform_segments(
                        body.declarations.last_mut().expect("terminal body"),
                        self,
                    );
                    self.emit_named_struct(&segments, &body, 0)?;
                    line(
                        0,
                        &mut self.out,
                        format_args!("impl From<{name}> for {segments} {{"),
                    );
                    line(
                        4,
                        &mut self.out,
                        format_args!("fn from(value: {name}) -> Self {{ Self {{"),
                    );
                    for (index, declaration) in item.body.declarations.iter().enumerate() {
                        let field = rust_ident(&declaration.declarator.name);
                        let access = format!("value.{field}");
                        let expression = if index + 1 == item.body.declarations.len() {
                            layouts.conversion(declaration, &access)
                        } else {
                            access
                        };
                        line(8, &mut self.out, format_args!("{field}: {expression},"));
                    }
                    line(4, &mut self.out, format_args!("}} }}"));
                    line(0, &mut self.out, format_args!("}}"));
                    self.out.push('\n');
                }
                Item::Union(item) => {
                    let mut body = item.body.clone();
                    for arm in &mut body.arms {
                        if layouts.terminal(
                            &arm.declaration.type_spec,
                            &arm.declaration.declarator.modifier,
                            &mut HashSet::new(),
                        ) {
                            layouts.transform_segments(&mut arm.declaration, self);
                        }
                    }
                    self.emit_named_union(&segments, &body, 0)?;
                    line(
                        0,
                        &mut self.out,
                        format_args!("impl From<{name}> for {segments} {{"),
                    );
                    line(
                        4,
                        &mut self.out,
                        format_args!("fn from(value: {name}) -> Self {{ match value {{"),
                    );
                    for arm in &item.body.arms {
                        let field = rust_ident(&arm.declaration.declarator.name);
                        let is_void = matches!(arm.declaration.type_spec, TypeSpec::Void);
                        let terminal = layouts.terminal(
                            &arm.declaration.type_spec,
                            &arm.declaration.declarator.modifier,
                            &mut HashSet::new(),
                        );
                        let expression = if terminal {
                            layouts.conversion(&arm.declaration, &field)
                        } else {
                            field.clone()
                        };
                        for label in &arm.labels {
                            let variant = union_variant_name(label);
                            let (pattern, construct) = if matches!(label, UnionCaseLabel::Default) {
                                if is_void {
                                    (
                                        " { discriminant }".to_string(),
                                        " { discriminant }".to_string(),
                                    )
                                } else {
                                    (
                                        format!(" {{ discriminant, {field} }}"),
                                        if terminal {
                                            format!(" {{ discriminant, {field}: {expression} }}")
                                        } else {
                                            format!(" {{ discriminant, {field} }}")
                                        },
                                    )
                                }
                            } else if is_void {
                                (String::new(), String::new())
                            } else {
                                (
                                    format!(" {{ {field} }}"),
                                    if terminal {
                                        format!(" {{ {field}: {expression} }}")
                                    } else {
                                        format!(" {{ {field} }}")
                                    },
                                )
                            };
                            line(
                                8,
                                &mut self.out,
                                format_args!(
                                    "{name}::{variant}{pattern} => Self::{variant}{construct},"
                                ),
                            );
                        }
                    }
                    line(4, &mut self.out, format_args!("}} }}"));
                    line(0, &mut self.out, format_args!("}}"));
                    self.out.push('\n');
                }
                _ => unreachable!(),
            }
        }
        Ok(())
    }

    fn emit_metadata_trait(&mut self, name: &str, prefix: usize, expression: &str) {
        line(
            0,
            &mut self.out,
            format_args!("impl onc_rpc_xdr::XdrReplyMetadata for {name} {{"),
        );
        line(
            4,
            &mut self.out,
            format_args!("const MAX_PREFIX_BYTES: usize = {prefix};"),
        );
        line(
            4,
            &mut self.out,
            format_args!("fn opaque_len(&self) -> Option<usize> {{ {expression} }}"),
        );
        line(0, &mut self.out, format_args!("}}"));
        self.out.push('\n');
    }

    pub(super) fn emit_segmented_enum(&mut self, name: &str, indent: usize) {
        self.segmented_start(name, indent);
        line(
            indent + 8,
            &mut self.out,
            format_args!("output.encode(self)"),
        );
        self.segmented_end(indent);
    }

    fn segmented_start(&mut self, name: &str, indent: usize) {
        line(
            indent,
            &mut self.out,
            format_args!("impl onc_rpc_xdr::XdrEncodeSegments for {name} {{"),
        );
        line(
            indent + 4,
            &mut self.out,
            format_args!(
                "fn encode_xdr_segments(&self, output: &mut onc_rpc_xdr::SegmentEncoder) -> Result<(), onc_rpc_xdr::XdrError> {{"
            ),
        );
    }
    fn segmented_end(&mut self, indent: usize) {
        line(indent + 4, &mut self.out, format_args!("}}"));
        line(indent, &mut self.out, format_args!("}}"));
        self.out.push('\n');
    }

    pub(super) fn emit_segmented_struct(
        &mut self,
        name: &str,
        body: &StructBody,
        indent: usize,
    ) -> Result<(), GeneratorError> {
        self.segmented_start(name, indent);
        for declaration in &body.declarations {
            self.segmented_declaration(
                &declaration.type_spec,
                &declaration.declarator,
                &format!("self.{}", rust_ident(&declaration.declarator.name)),
                indent + 8,
            )?;
        }
        line(indent + 8, &mut self.out, format_args!("Ok(())"));
        self.segmented_end(indent);
        Ok(())
    }

    pub(super) fn emit_segmented_union(
        &mut self,
        name: &str,
        body: &UnionBody,
        indent: usize,
    ) -> Result<(), GeneratorError> {
        self.segmented_start(name, indent);
        line(indent + 8, &mut self.out, format_args!("match self {{"));
        for arm in &body.arms {
            let field = rust_ident(&arm.declaration.declarator.name);
            let is_void = matches!(arm.declaration.type_spec, TypeSpec::Void);
            for label in &arm.labels {
                let variant = union_variant_name(label);
                let default = matches!(label, UnionCaseLabel::Default);
                let pattern = if default {
                    format!(
                        "Self::{variant} {{ discriminant{} }}",
                        if is_void {
                            String::new()
                        } else {
                            format!(", {field}")
                        }
                    )
                } else if is_void {
                    format!("Self::{variant}")
                } else {
                    format!("Self::{variant} {{ {field} }}")
                };
                line(indent + 12, &mut self.out, format_args!("{pattern} => {{"));
                if default {
                    line(
                        indent + 16,
                        &mut self.out,
                        format_args!("output.encode(discriminant)?;"),
                    );
                } else {
                    let disc = self.render_union_label(label, &body.discriminant.type_spec);
                    line(
                        indent + 16,
                        &mut self.out,
                        format_args!("output.encode(&({disc}))?;"),
                    );
                }
                if !is_void {
                    self.segmented_declaration(
                        &arm.declaration.type_spec,
                        &arm.declaration.declarator,
                        &field,
                        indent + 16,
                    )?;
                }
                line(indent + 16, &mut self.out, format_args!("Ok(())"));
                line(indent + 12, &mut self.out, format_args!("}}"));
            }
        }
        line(indent + 8, &mut self.out, format_args!("}}"));
        self.segmented_end(indent);
        Ok(())
    }

    fn opaque_alias_bound(
        &self,
        target: &TypeSpec,
        seen: &mut HashSet<String>,
    ) -> Option<Option<ValueExpr>> {
        let TypeSpec::Identifier(name) = target else {
            return None;
        };
        if self.segment_projection_names.contains(name) {
            return None;
        }
        if !seen.insert(name.clone()) {
            return None;
        }
        match self.named_types.get(name)? {
            NamedType::Typedef {
                target: TypeSpec::Opaque,
                modifier: Some(DeclaratorModifier::VariableArray(bound)),
            } => Some(bound.clone()),
            NamedType::Typedef {
                target,
                modifier: None,
            } => self.opaque_alias_bound(target, seen),
            _ => None,
        }
    }

    fn segmented_declaration(
        &mut self,
        target: &TypeSpec,
        declarator: &Declarator,
        access: &str,
        indent: usize,
    ) -> Result<(), GeneratorError> {
        let opaque_bound = match (target, &declarator.modifier) {
            (TypeSpec::Opaque, Some(DeclaratorModifier::VariableArray(bound))) => {
                Some(bound.clone())
            }
            (_, None) => self.opaque_alias_bound(target, &mut HashSet::new()),
            _ => None,
        };
        if let Some(bound) = opaque_bound {
            let maximum = bound
                .map(|bound| format!("Some({} as usize)", self.render_value(&bound)))
                .unwrap_or_else(|| "None".into());
            line(
                indent,
                &mut self.out,
                format_args!("output.opaque(&{access}, {maximum})?;"),
            );
            return Ok(());
        }
        if let Some(DeclaratorModifier::FixedArray(_)) = &declarator.modifier {
            if matches!(target, TypeSpec::Opaque) {
                line(
                    indent,
                    &mut self.out,
                    format_args!("output.encode(&{access})?;"),
                );
            } else {
                line(
                    indent,
                    &mut self.out,
                    format_args!(
                        "for value in &{access} {{ onc_rpc_xdr::XdrEncodeSegments::encode_xdr_segments(value, output)?; }}"
                    ),
                );
            }
        } else {
            line(
                indent,
                &mut self.out,
                format_args!(
                    "onc_rpc_xdr::XdrEncodeSegments::encode_xdr_segments(&{access}, output)?;"
                ),
            );
        }
        Ok(())
    }
}

impl StubEmitter {
    pub(super) fn validate_owned_method_names(
        &self,
        version: &VersionDecl,
    ) -> Result<(), GeneratorError> {
        let mut names = BTreeSet::from(["new".to_string()]);
        for procedure in &version.procedures {
            let method = to_snake_case(&procedure.name);
            names.insert(method.clone());
            names.insert(format!("{method}_with_options"));
        }
        for procedure in &version.procedures {
            let method = to_snake_case(&procedure.name);
            let mut owned = vec![
                format!("{method}_segmented"),
                format!("try_{method}_segmented"),
            ];
            if matches!(&procedure.return_type, TypeSpec::Identifier(name) if self.owned_layouts.metadata_name(name).is_some())
            {
                owned.extend([format!("{method}_into"), format!("try_{method}_into")]);
            }
            for name in owned {
                if !names.insert(name.clone()) {
                    return Err(GeneratorError::UnsupportedConstruct(format!(
                        "generated caller-owned method name collides in version {}: {name}",
                        version.name
                    )));
                }
            }
        }
        Ok(())
    }

    fn sibling_type(&self, name: &str, sibling: &str) -> String {
        match (&self.current_module, self.type_owners.get(name)) {
            (Some(_), Some(owner)) => format!("crate::{owner}::{sibling}"),
            _ => sibling.to_string(),
        }
    }

    pub(super) fn emit_owned_client_methods(
        &mut self,
        procedure: &ProcedureDecl,
        indent: usize,
    ) -> Result<(), GeneratorError> {
        let method = to_snake_case(&procedure.name);
        let argument =
            self.procedure_rust_type(&procedure.argument_type, &format!("{method}_arg"), indent)?;
        let reply =
            self.procedure_rust_type(&procedure.return_type, &format!("{method}_ret"), indent)?;
        let projected = match &procedure.argument_type {
            TypeSpec::Identifier(name) => self
                .owned_layouts
                .segments_name(name)
                .map(|segments| self.sibling_type(name, &segments)),
            _ => None,
        };
        let owned_argument = projected.clone().unwrap_or_else(|| argument.clone());
        let (arg_param, arg_generic, binding, captures) = if let Some(projected) = projected {
            (
                "argument: A, ".to_string(),
                format!(", A: Into<{projected}>"),
                format!("let argument: {projected} = argument.into();"),
                "A, B",
            )
        } else {
            (
                if argument == "()" {
                    String::new()
                } else {
                    format!("argument: {argument}, ")
                },
                String::new(),
                String::new(),
                "B",
            )
        };
        let arg_access = if argument == "()" { "&()" } else { "&argument" };
        let call = format!(
            "onc_rpc_runtime::ProgramVersion {{ program: super::super::PROGRAM, version: super::VERSION }}, onc_rpc_runtime::Procedure(super::{}), {arg_access}, buffer, options",
            procedure.name
        );
        let try_call = call.replace(
            &format!(", {arg_access}, buffer, options"),
            &format!(
                ", {}, buffer, options",
                if argument == "()" { "()" } else { "argument" }
            ),
        );
        self.emit_method_docs(
            indent,
            &format!(
                "Call RPC `{}` with segmented encoding and a caller-owned reply buffer.",
                procedure.name
            ),
            "Decodes the full typed reply; the outcome returns `buffer`, including on error.",
        );
        line(
            indent,
            &mut self.out,
            format_args!(
                "pub async fn {method}_segmented<B: AsMut<[u8]> + Send + 'static{arg_generic}>(&self, {arg_param}buffer: B, options: &onc_rpc_runtime::CallOptions) -> onc_rpc_runtime::TypedReceiveOutcome<{reply}, B> {{"
            ),
        );
        if !binding.is_empty() {
            line(indent + 4, &mut self.out, format_args!("{binding}"));
        }
        line(
            indent + 4,
            &mut self.out,
            format_args!("self.client.call_xdr_segmented::<_, {reply}, B>({call}).await"),
        );
        line(indent, &mut self.out, format_args!("}}"));
        self.emit_method_docs(
            indent,
            &format!(
                "Admit RPC `{}` with segmented encoding, without waiting for capacity.",
                procedure.name
            ),
            "Returns an unpolled future, or the unsent request and buffer on refusal.",
        );
        line(
            indent,
            &mut self.out,
            format_args!(
                "#[allow(clippy::result_large_err)] // Refusal returns original owners without boxing."
            ),
        );
        line(
            indent,
            &mut self.out,
            format_args!(
                "pub fn try_{method}_segmented<B: AsMut<[u8]> + Send + 'static{arg_generic}>(&self, {arg_param}buffer: B, options: &onc_rpc_runtime::CallOptions) -> Result<impl std::future::Future<Output = onc_rpc_runtime::TypedReceiveOutcome<{reply}, B>> + Send + 'static + use<{captures}>, onc_rpc_runtime::CallRefusal<{owned_argument}, B>> {{"
            ),
        );
        if !binding.is_empty() {
            line(indent + 4, &mut self.out, format_args!("{binding}"));
        }
        line(
            indent + 4,
            &mut self.out,
            format_args!("self.client.try_call_xdr_segmented::<_, {reply}, B>({try_call})"),
        );
        line(indent, &mut self.out, format_args!("}}"));
        if let TypeSpec::Identifier(name) = &procedure.return_type {
            if let Some(metadata_name) = self.owned_layouts.metadata_name(name) {
                let metadata = self.sibling_type(name, &metadata_name);
                self.emit_method_docs(
                    indent,
                    &format!(
                        "Call RPC `{}`, placing the reply's opaque body directly in `buffer`.",
                        procedure.name
                    ),
                    "Returns typed metadata and the quiescent buffer, including on error.",
                );
                line(
                    indent,
                    &mut self.out,
                    format_args!(
                        "pub async fn {method}_into<B: AsMut<[u8]> + Send + 'static{arg_generic}>(&self, {arg_param}buffer: B, options: &onc_rpc_runtime::CallOptions) -> onc_rpc_runtime::TypedReceiveOutcome<{metadata}, B> {{"
                    ),
                );
                if !binding.is_empty() {
                    line(indent + 4, &mut self.out, format_args!("{binding}"));
                }
                line(
                    indent + 4,
                    &mut self.out,
                    format_args!("self.client.call_xdr_into::<_, {metadata}, B>({call}).await"),
                );
                line(indent, &mut self.out, format_args!("}}"));
                self.emit_method_docs(
                    indent,
                    &format!(
                        "Admit RPC `{}` for direct-body receive, without waiting for capacity.",
                        procedure.name
                    ),
                    "Returns an unpolled future, or the unsent request and buffer on refusal.",
                );
                line(
                    indent,
                    &mut self.out,
                    format_args!(
                        "#[allow(clippy::result_large_err)] // Refusal returns original owners without boxing."
                    ),
                );
                line(
                    indent,
                    &mut self.out,
                    format_args!(
                        "pub fn try_{method}_into<B: AsMut<[u8]> + Send + 'static{arg_generic}>(&self, {arg_param}buffer: B, options: &onc_rpc_runtime::CallOptions) -> Result<impl std::future::Future<Output = onc_rpc_runtime::TypedReceiveOutcome<{metadata}, B>> + Send + 'static + use<{captures}>, onc_rpc_runtime::CallRefusal<{owned_argument}, B>> {{"
                    ),
                );
                if !binding.is_empty() {
                    line(indent + 4, &mut self.out, format_args!("{binding}"));
                }
                line(
                    indent + 4,
                    &mut self.out,
                    format_args!("self.client.try_call_xdr_into::<_, {metadata}, B>({try_call})"),
                );
                line(indent, &mut self.out, format_args!("}}"));
            }
        }
        self.out.push('\n');
        Ok(())
    }
}
