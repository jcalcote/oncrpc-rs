use crate::GeneratorError;
use crate::ast::*;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Keyword(Keyword),
    Identifier(String),
    Number(i64),
    Symbol(char),
    Eof,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Keyword {
    Bool,
    Case,
    Const,
    Default,
    Double,
    Enum,
    Float,
    Hyper,
    Int,
    Opaque,
    Program,
    Quadruple,
    String,
    Struct,
    Switch,
    Typedef,
    Union,
    Unsigned,
    Version,
    Void,
}

pub fn parse_x_source(input: &str) -> Result<Schema, GeneratorError> {
    let cleaned = strip_unsupported_lines(strip_block_comments(input)?);
    let tokens = tokenize(&cleaned)?;
    Parser::new(tokens).parse_schema()
}

fn strip_block_comments(input: &str) -> Result<String, GeneratorError> {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut idx = 0;

    while idx < bytes.len() {
        if idx + 1 < bytes.len() && bytes[idx] == b'/' && bytes[idx + 1] == b'*' {
            idx += 2;
            let mut closed = false;
            while idx + 1 < bytes.len() {
                if bytes[idx] == b'*' && bytes[idx + 1] == b'/' {
                    idx += 2;
                    closed = true;
                    break;
                }
                idx += 1;
            }
            if !closed {
                return Err(GeneratorError::Parse(
                    "unterminated block comment in XDR source".to_string(),
                ));
            }
        } else {
            out.push(bytes[idx] as char);
            idx += 1;
        }
    }

    Ok(out)
}

fn strip_unsupported_lines(input: String) -> String {
    input
        .lines()
        .filter(|line| !line.trim_start().starts_with('%'))
        .collect::<Vec<_>>()
        .join("\n")
}

fn tokenize(input: &str) -> Result<Vec<Token>, GeneratorError> {
    let mut tokens = Vec::new();
    let mut chars = input.char_indices().peekable();

    while let Some((idx, ch)) = chars.peek().copied() {
        if ch.is_whitespace() {
            chars.next();
            continue;
        }

        if ch.is_ascii_alphabetic() || ch == '_' {
            let start = idx;
            chars.next();
            let mut end = start + ch.len_utf8();
            while let Some((next_idx, next_ch)) = chars.peek().copied() {
                if next_ch.is_ascii_alphanumeric() || next_ch == '_' {
                    chars.next();
                    end = next_idx + next_ch.len_utf8();
                } else {
                    break;
                }
            }
            let ident = &input[start..end];
            tokens.push(match keyword(ident) {
                Some(keyword) => Token::Keyword(keyword),
                None => Token::Identifier(ident.to_string()),
            });
            continue;
        }

        if ch.is_ascii_digit() || ch == '-' {
            let start = idx;
            chars.next();
            let mut end = start + ch.len_utf8();
            while let Some((next_idx, next_ch)) = chars.peek().copied() {
                if next_ch.is_ascii_alphanumeric() || next_ch == '_' {
                    chars.next();
                    end = next_idx + next_ch.len_utf8();
                } else {
                    break;
                }
            }
            let text = &input[start..end];
            tokens.push(Token::Number(parse_number(text)?));
            continue;
        }

        if "{}()<>[];,=:*".contains(ch) {
            chars.next();
            tokens.push(Token::Symbol(ch));
            continue;
        }

        return Err(GeneratorError::Parse(format!(
            "unsupported character '{ch}' in XDR source"
        )));
    }

    tokens.push(Token::Eof);
    Ok(tokens)
}

fn parse_number(text: &str) -> Result<i64, GeneratorError> {
    let (negative, digits) = if let Some(rest) = text.strip_prefix('-') {
        (true, rest)
    } else {
        (false, text)
    };

    let (radix, body) = if let Some(hex) = digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
    {
        (16, hex)
    } else if digits.len() > 1 && digits.starts_with('0') {
        (8, &digits[1..])
    } else {
        (10, digits)
    };

    let mut value = i64::from_str_radix(body, radix)
        .map_err(|_| GeneratorError::Parse(format!("invalid integer literal {text}")))?;
    if negative {
        value = -value;
    }
    Ok(value)
}

fn keyword(text: &str) -> Option<Keyword> {
    Some(match text {
        "bool" => Keyword::Bool,
        "case" => Keyword::Case,
        "const" => Keyword::Const,
        "default" => Keyword::Default,
        "double" => Keyword::Double,
        "enum" => Keyword::Enum,
        "float" => Keyword::Float,
        "hyper" => Keyword::Hyper,
        "int" => Keyword::Int,
        "opaque" => Keyword::Opaque,
        "program" => Keyword::Program,
        "quadruple" => Keyword::Quadruple,
        "string" => Keyword::String,
        "struct" => Keyword::Struct,
        "switch" => Keyword::Switch,
        "typedef" => Keyword::Typedef,
        "union" => Keyword::Union,
        "unsigned" => Keyword::Unsigned,
        "version" => Keyword::Version,
        "void" => Keyword::Void,
        _ => return None,
    })
}

struct Parser {
    tokens: Vec<Token>,
    idx: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, idx: 0 }
    }

    fn parse_schema(mut self) -> Result<Schema, GeneratorError> {
        let mut items = Vec::new();

        while !matches!(self.peek(), Token::Eof) {
            items.push(self.parse_item()?);
        }

        Ok(Schema { items })
    }

    fn parse_item(&mut self) -> Result<Item, GeneratorError> {
        match self.peek() {
            Token::Keyword(Keyword::Const) => self.parse_const().map(Item::Const),
            Token::Keyword(Keyword::Typedef) => self.parse_typedef().map(Item::Typedef),
            Token::Keyword(Keyword::Struct) => self.parse_named_struct().map(Item::Struct),
            Token::Keyword(Keyword::Enum) => self.parse_named_enum().map(Item::Enum),
            Token::Keyword(Keyword::Union) => self.parse_named_union().map(Item::Union),
            Token::Keyword(Keyword::Program) => self.parse_program().map(Item::Program),
            other => Err(GeneratorError::Parse(format!(
                "unexpected top-level token {other:?}"
            ))),
        }
    }

    fn parse_const(&mut self) -> Result<ConstDecl, GeneratorError> {
        self.expect_keyword(Keyword::Const)?;
        let name = self.expect_identifier()?;
        self.expect_symbol('=')?;
        let value = self.parse_value_expr()?;
        self.expect_symbol(';')?;
        Ok(ConstDecl { name, value })
    }

    fn parse_typedef(&mut self) -> Result<TypedefDecl, GeneratorError> {
        self.expect_keyword(Keyword::Typedef)?;
        let target = self.parse_type_spec()?;
        let declarator = self.parse_declarator()?;
        self.expect_symbol(';')?;
        Ok(TypedefDecl { target, declarator })
    }

    fn parse_named_struct(&mut self) -> Result<StructDecl, GeneratorError> {
        self.expect_keyword(Keyword::Struct)?;
        let name = self.expect_identifier()?;
        let body = self.parse_struct_body()?;
        self.expect_symbol(';')?;
        Ok(StructDecl { name, body })
    }

    fn parse_named_enum(&mut self) -> Result<EnumDecl, GeneratorError> {
        self.expect_keyword(Keyword::Enum)?;
        let name = self.expect_identifier()?;
        let body = self.parse_enum_body()?;
        self.expect_symbol(';')?;
        Ok(EnumDecl { name, body })
    }

    fn parse_named_union(&mut self) -> Result<UnionDecl, GeneratorError> {
        self.expect_keyword(Keyword::Union)?;
        let name = self.expect_identifier()?;
        let body = self.parse_union_body()?;
        self.expect_symbol(';')?;
        Ok(UnionDecl { name, body })
    }

    fn parse_program(&mut self) -> Result<ProgramDecl, GeneratorError> {
        self.expect_keyword(Keyword::Program)?;
        let name = self.expect_identifier()?;
        self.expect_symbol('{')?;
        let mut versions = Vec::new();

        while !self.consume_symbol('}') {
            versions.push(self.parse_version()?);
        }

        self.expect_symbol('=')?;
        let number = self.expect_u32()?;
        self.expect_symbol(';')?;

        Ok(ProgramDecl {
            name,
            versions,
            number,
        })
    }

    fn parse_version(&mut self) -> Result<VersionDecl, GeneratorError> {
        self.expect_keyword(Keyword::Version)?;
        let name = self.expect_identifier()?;
        self.expect_symbol('{')?;
        let mut procedures = Vec::new();

        while !self.consume_symbol('}') {
            procedures.push(self.parse_procedure()?);
        }

        self.expect_symbol('=')?;
        let number = self.expect_u32()?;
        self.expect_symbol(';')?;

        Ok(VersionDecl {
            name,
            procedures,
            number,
        })
    }

    fn parse_procedure(&mut self) -> Result<ProcedureDecl, GeneratorError> {
        let return_type = self.parse_type_spec()?;
        let name = self.expect_identifier()?;
        self.expect_symbol('(')?;
        let argument_type = self.parse_procedure_argument_type()?;
        self.expect_symbol(')')?;
        self.expect_symbol('=')?;
        let number = self.expect_u32()?;
        self.expect_symbol(';')?;

        Ok(ProcedureDecl {
            return_type,
            name,
            argument_type,
            number,
        })
    }

    fn parse_procedure_argument_type(&mut self) -> Result<TypeSpec, GeneratorError> {
        if matches!(self.peek(), Token::Keyword(Keyword::Void)) {
            self.next();
            return Ok(TypeSpec::Void);
        }

        let argument_type = self.parse_type_spec()?;
        if !matches!(self.peek(), Token::Symbol(')')) {
            let _ = self.parse_declarator()?;
        }
        Ok(argument_type)
    }

    fn parse_type_spec(&mut self) -> Result<TypeSpec, GeneratorError> {
        match self.next() {
            Token::Keyword(Keyword::Void) => Ok(TypeSpec::Void),
            Token::Keyword(Keyword::Bool) => Ok(TypeSpec::Bool),
            Token::Keyword(Keyword::Int) => Ok(TypeSpec::Int),
            Token::Keyword(Keyword::Hyper) => Ok(TypeSpec::Hyper),
            Token::Keyword(Keyword::Float) => Ok(TypeSpec::Float),
            Token::Keyword(Keyword::Double) => Ok(TypeSpec::Double),
            Token::Keyword(Keyword::Quadruple) => Ok(TypeSpec::Quadruple),
            Token::Keyword(Keyword::Opaque) => Ok(TypeSpec::Opaque),
            Token::Keyword(Keyword::String) => Ok(TypeSpec::String),
            Token::Keyword(Keyword::Unsigned) => match self.next() {
                Token::Keyword(Keyword::Int) => Ok(TypeSpec::UnsignedInt),
                Token::Keyword(Keyword::Hyper) => Ok(TypeSpec::UnsignedHyper),
                other => Err(GeneratorError::Parse(format!(
                    "expected int or hyper after unsigned, got {other:?}"
                ))),
            },
            Token::Identifier(name) => Ok(TypeSpec::Identifier(name)),
            Token::Keyword(Keyword::Enum) => {
                if let Token::Identifier(name) = self.peek().clone() {
                    if !matches!(self.peek_n(1), Token::Symbol('{')) {
                        self.next();
                        Ok(TypeSpec::Identifier(name))
                    } else {
                        Ok(TypeSpec::Enum(self.parse_enum_body()?))
                    }
                } else {
                    Ok(TypeSpec::Enum(self.parse_enum_body()?))
                }
            }
            Token::Keyword(Keyword::Struct) => {
                if let Token::Identifier(name) = self.peek().clone() {
                    if !matches!(self.peek_n(1), Token::Symbol('{')) {
                        self.next();
                        Ok(TypeSpec::Identifier(name))
                    } else {
                        Ok(TypeSpec::Struct(Box::new(self.parse_struct_body()?)))
                    }
                } else {
                    Ok(TypeSpec::Struct(Box::new(self.parse_struct_body()?)))
                }
            }
            Token::Keyword(Keyword::Union) => {
                if let Token::Identifier(name) = self.peek().clone() {
                    if !matches!(self.peek_n(1), Token::Keyword(Keyword::Switch)) {
                        self.next();
                        Ok(TypeSpec::Identifier(name))
                    } else {
                        Ok(TypeSpec::Union(Box::new(self.parse_union_body()?)))
                    }
                } else {
                    Ok(TypeSpec::Union(Box::new(self.parse_union_body()?)))
                }
            }
            Token::Keyword(other) => Err(GeneratorError::Parse(format!(
                "unsupported keyword in type position: {other:?}"
            ))),
            other => Err(GeneratorError::Parse(format!(
                "unexpected token in type position: {other:?}"
            ))),
        }
    }

    fn parse_struct_body(&mut self) -> Result<StructBody, GeneratorError> {
        self.expect_symbol('{')?;
        let mut declarations = Vec::new();

        while !self.consume_symbol('}') {
            declarations.push(self.parse_declaration()?);
            self.expect_symbol(';')?;
        }

        Ok(StructBody { declarations })
    }

    fn parse_enum_body(&mut self) -> Result<EnumBody, GeneratorError> {
        self.expect_symbol('{')?;
        let mut variants = Vec::new();

        while !self.consume_symbol('}') {
            let name = self.expect_identifier()?;
            self.expect_symbol('=')?;
            let value = self.parse_value_expr()?;
            variants.push(EnumVariant { name, value });
            let _ = self.consume_symbol(',');
        }

        Ok(EnumBody { variants })
    }

    fn parse_union_body(&mut self) -> Result<UnionBody, GeneratorError> {
        self.expect_keyword(Keyword::Switch)?;
        self.expect_symbol('(')?;
        let discriminant = self.parse_declaration()?;
        self.expect_symbol(')')?;
        self.expect_symbol('{')?;
        let mut arms = Vec::new();

        while !self.consume_symbol('}') {
            let mut labels = Vec::new();
            loop {
                match self.peek() {
                    Token::Keyword(Keyword::Case) => {
                        self.next();
                        let value = self.parse_value_expr()?;
                        self.expect_symbol(':')?;
                        labels.push(UnionCaseLabel::Case(value));
                    }
                    Token::Keyword(Keyword::Default) => {
                        self.next();
                        self.expect_symbol(':')?;
                        labels.push(UnionCaseLabel::Default);
                    }
                    _ => break,
                }
            }

            if labels.is_empty() {
                return Err(GeneratorError::Parse(
                    "expected case or default label in union arm".to_string(),
                ));
            }

            let declaration = self.parse_declaration()?;
            self.expect_symbol(';')?;
            arms.push(UnionArm {
                labels,
                declaration,
            });
        }

        Ok(UnionBody { discriminant, arms })
    }

    fn parse_declaration(&mut self) -> Result<Declaration, GeneratorError> {
        let type_spec = self.parse_type_spec()?;
        let declarator = if matches!(type_spec, TypeSpec::Void)
            && matches!(self.peek(), Token::Symbol(';') | Token::Symbol(')'))
        {
            Declarator {
                name: String::new(),
                modifier: None,
            }
        } else {
            self.parse_declarator()?
        };
        Ok(Declaration {
            type_spec,
            declarator,
        })
    }

    fn parse_declarator(&mut self) -> Result<Declarator, GeneratorError> {
        let optional = self.consume_symbol('*');
        let name = self.expect_identifier()?;
        let modifier = if self.consume_symbol('[') {
            let bound = self.parse_value_expr()?;
            self.expect_symbol(']')?;
            Some(DeclaratorModifier::FixedArray(bound))
        } else if self.consume_symbol('<') {
            let bound = if self.consume_symbol('>') {
                None
            } else {
                let bound = self.parse_value_expr()?;
                self.expect_symbol('>')?;
                Some(bound)
            };
            Some(DeclaratorModifier::VariableArray(bound))
        } else if optional {
            Some(DeclaratorModifier::Optional)
        } else {
            None
        };

        Ok(Declarator { name, modifier })
    }

    fn parse_value_expr(&mut self) -> Result<ValueExpr, GeneratorError> {
        match self.next() {
            Token::Number(value) => Ok(ValueExpr::Number(value)),
            Token::Identifier(name) => Ok(ValueExpr::Identifier(name)),
            other => Err(GeneratorError::Parse(format!(
                "unexpected token in value expression: {other:?}"
            ))),
        }
    }

    fn expect_keyword(&mut self, keyword: Keyword) -> Result<(), GeneratorError> {
        match self.next() {
            Token::Keyword(found) if found == keyword => Ok(()),
            other => Err(GeneratorError::Parse(format!(
                "expected keyword {keyword:?}, got {other:?}"
            ))),
        }
    }

    fn expect_identifier(&mut self) -> Result<String, GeneratorError> {
        match self.next() {
            Token::Identifier(name) => Ok(name),
            other => Err(GeneratorError::Parse(format!(
                "expected identifier, got {other:?}"
            ))),
        }
    }

    fn expect_symbol(&mut self, symbol: char) -> Result<(), GeneratorError> {
        match self.next() {
            Token::Symbol(found) if found == symbol => Ok(()),
            other => Err(GeneratorError::Parse(format!(
                "expected symbol '{symbol}', got {other:?}"
            ))),
        }
    }

    fn expect_u32(&mut self) -> Result<u32, GeneratorError> {
        match self.next() {
            Token::Number(value) if value >= 0 && value <= u32::MAX as i64 => Ok(value as u32),
            other => Err(GeneratorError::Parse(format!(
                "expected non-negative integer that fits in u32, got {other:?}"
            ))),
        }
    }

    fn consume_symbol(&mut self, symbol: char) -> bool {
        match self.peek() {
            Token::Symbol(found) if *found == symbol => {
                self.idx += 1;
                true
            }
            _ => false,
        }
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.idx]
    }

    fn peek_n(&self, offset: usize) -> &Token {
        &self.tokens[self.idx + offset]
    }

    fn next(&mut self) -> Token {
        let token = self.tokens[self.idx].clone();
        self.idx += 1;
        token
    }
}
