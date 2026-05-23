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
    Const,
    Enum,
    Hyper,
    Int,
    Opaque,
    Program,
    String,
    Struct,
    Typedef,
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
                if next_ch.is_ascii_hexdigit() || matches!(next_ch, 'x' | 'X') {
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

        if "{}()<>;,=".contains(ch) {
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
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        i64::from_str_radix(hex, 16)
            .map_err(|_| GeneratorError::Parse(format!("invalid hex literal {text}")))
    } else if let Some(hex) = text
        .strip_prefix("-0x")
        .or_else(|| text.strip_prefix("-0X"))
    {
        i64::from_str_radix(hex, 16)
            .map(|value| -value)
            .map_err(|_| GeneratorError::Parse(format!("invalid hex literal {text}")))
    } else {
        text.parse::<i64>()
            .map_err(|_| GeneratorError::Parse(format!("invalid integer literal {text}")))
    }
}

fn keyword(text: &str) -> Option<Keyword> {
    Some(match text {
        "bool" => Keyword::Bool,
        "const" => Keyword::Const,
        "enum" => Keyword::Enum,
        "hyper" => Keyword::Hyper,
        "int" => Keyword::Int,
        "opaque" => Keyword::Opaque,
        "program" => Keyword::Program,
        "string" => Keyword::String,
        "struct" => Keyword::Struct,
        "typedef" => Keyword::Typedef,
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
            Token::Keyword(Keyword::Struct) => self.parse_struct().map(Item::Struct),
            Token::Keyword(Keyword::Enum) => self.parse_enum().map(Item::Enum),
            Token::Keyword(Keyword::Program) => self.parse_program().map(Item::Program),
            Token::Identifier(name) if name == "union" => {
                Err(GeneratorError::UnsupportedConstruct(
                    "union declarations are not supported yet".to_string(),
                ))
            }
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

    fn parse_struct(&mut self) -> Result<StructDecl, GeneratorError> {
        self.expect_keyword(Keyword::Struct)?;
        let name = self.expect_identifier()?;
        self.expect_symbol('{')?;
        let mut fields = Vec::new();

        while !self.consume_symbol('}') {
            let field_type = self.parse_type_spec()?;
            let declarator = self.parse_declarator()?;
            self.expect_symbol(';')?;
            fields.push(FieldDecl {
                field_type,
                declarator,
            });
        }

        self.expect_symbol(';')?;
        Ok(StructDecl { name, fields })
    }

    fn parse_enum(&mut self) -> Result<EnumDecl, GeneratorError> {
        self.expect_keyword(Keyword::Enum)?;
        let name = self.expect_identifier()?;
        self.expect_symbol('{')?;
        let mut variants = Vec::new();

        while !self.consume_symbol('}') {
            let variant_name = self.expect_identifier()?;
            self.expect_symbol('=')?;
            let value = self.parse_value_expr()?;
            variants.push(EnumVariant {
                name: variant_name,
                value,
            });
            let _ = self.consume_symbol(',');
        }

        self.expect_symbol(';')?;
        Ok(EnumDecl { name, variants })
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
        let argument_type = self.parse_type_spec()?;
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

    fn parse_type_spec(&mut self) -> Result<TypeSpec, GeneratorError> {
        match self.next() {
            Token::Keyword(Keyword::Void) => Ok(TypeSpec::Void),
            Token::Keyword(Keyword::Bool) => Ok(TypeSpec::Bool),
            Token::Keyword(Keyword::Int) => Ok(TypeSpec::Int),
            Token::Keyword(Keyword::Hyper) => Ok(TypeSpec::Hyper),
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
            Token::Keyword(other) => Err(GeneratorError::Parse(format!(
                "unsupported keyword in type position: {other:?}"
            ))),
            other => Err(GeneratorError::Parse(format!(
                "unexpected token in type position: {other:?}"
            ))),
        }
    }

    fn parse_declarator(&mut self) -> Result<Declarator, GeneratorError> {
        let name = self.expect_identifier()?;
        let modifier = if self.consume_symbol('<') {
            let bound = if self.consume_symbol('>') {
                None
            } else {
                let bound = self.parse_value_expr()?;
                self.expect_symbol('>')?;
                Some(bound)
            };
            Some(DeclaratorModifier::VariableArray(bound))
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

    fn next(&mut self) -> Token {
        let token = self.tokens[self.idx].clone();
        self.idx += 1;
        token
    }
}
