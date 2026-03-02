//! `struct` schema parsing and reading.
//!
//! The schema spec is defined [here](https://github.com/wpilibsuite/allwpilib/blob/main/wpiutil/doc/struct.adoc).

use std::{collections::HashMap, iter::Peekable, str::Chars};

use crate::r#struct::byte::ByteReader;

/// A stream of tokens that can be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenStream {
    tokens: Vec<Token>,
}

impl TokenStream {
    /// Creates a new `TokenStream` from a vec of [`Token`]s.
    pub fn new(mut tokens: Vec<Token>) -> Self {
        // reverse to pop from front
        tokens.reverse();
        Self {
            tokens,
        }
    }

    /// Returns if the stream has tokens remaining.
    pub fn has_remaining(&mut self) -> bool {
        !self.tokens.is_empty()
    }

    /// Returns the next token, advancing the stream by one.
    ///
    /// # Errors
    /// Returns a [`ParseTokensError::Eof`] if there are no more tokens remaining.
    pub fn next_token(&mut self) -> Result<Token, ParseTokensError> {
        self.tokens.pop().ok_or(ParseTokensError::Eof)
    }

    /// Returns the next token if it is an identifier, advancing the stream by one.
    ///
    /// # Errors
    /// Returns an error if there are no more tokens remaining or if the next token is not an
    /// identifier.
    pub fn next_ident(&mut self) -> Result<String, ParseTokensError> {
        match self.next_token()? {
            Token::Ident(ident) => Ok(ident),
            _ => Err(ParseTokensError::Expected("identifier".to_owned())),
        }
    }

    /// Returns the next token if it is a number, advancing the stream by one.
    ///
    /// # Errors
    /// Returns an error if there are no more tokens remaining or if the next token is not a
    /// number.
    pub fn next_number(&mut self) -> Result<i32, ParseTokensError> {
        match self.next_token()? {
            Token::Number(num) => Ok(num),
            _ => Err(ParseTokensError::Expected("number".to_owned())),
        }
    }

    /// Peeks the next token, not advancing the stream.
    ///
    /// # Errors
    /// Returns a [`ParseTokensError::Eof`] if there are no more tokens remaining.
    pub fn peek(&mut self) -> Result<&Token, ParseTokensError> {
        self.tokens.last().ok_or(ParseTokensError::Eof)
    }

    /// Peeks the next token if it is an identifier, not advancing the stream.
    ///
    /// # Errors
    /// Returns an error if there are no more tokens remaining or if the next token is not an
    /// identifier.
    pub fn peek_ident(&mut self) -> Result<&String, ParseTokensError> {
        match self.peek()? {
            Token::Ident(ident) => Ok(ident),
            _ => Err(ParseTokensError::Expected("identifier".to_owned())),
        }
    }
}

/// Represents a parsed struct from a schema string.
///
/// # Examples
/// ```
/// use nt_client::r#struct::parse::parse_schema;
///
/// let schema = "int16 myint;float myfloat;bool mybool";
/// let parsed = parse_schema(schema).expect("valid schema");
/// assert_eq!(parsed.declarations.len(), 3);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedStruct {
    /// The declarations in the struct.
    pub declarations: Vec<StructDeclaration>,
    /// Dependencies of this struct.
    pub deps: Vec<String>,
}

impl ParsedStruct {
    /// Parses a stream of tokens into a `ParsedStruct`, as described [here](https://github.com/wpilibsuite/allwpilib/blob/main/wpiutil/doc/struct.adoc).
    pub fn parse_tokens(tokens: &mut TokenStream) -> Result<Self, ParseTokensError> {
        let mut declarations = Vec::new();
        let mut deps = Vec::new();

        declarations.push(StructDeclaration::parse_tokens(tokens)?);
        while tokens.has_remaining() {
            match tokens.next_token()? {
                Token::Semi => {},
                _ => return Err(ParseTokensError::Expected("`;`".to_owned())),
            }

            while let Ok(Token::Semi) = tokens.peek() {
                tokens.next_token()?;
            }
            if !tokens.has_remaining() { break; };

            let declaration = StructDeclaration::parse_tokens(tokens)?;
            if let TypeName::Struct(dep) = declaration.type_name() {
                deps.push(dep.clone());
            }
            declarations.push(declaration);
        }

        Ok(ParsedStruct { declarations, deps })
    }

    /// Reads this struct from byte data.
    ///
    /// The returned value contains a vector of both the field name and value.
    /// It is guaranteed that this vector will contain every single field with its respective type.
    pub fn read_from_bytes(&self, read: &mut ByteReader, parsed: &HashMap<String, ParsedStruct>) -> Option<Vec<(String, StructValue)>> {
        let mut fields = Vec::new();

        let mut current_bitfield: Option<(u32, u32, u64)> = None;
        for declaration in &self.declarations {
            let field = match declaration {
                StructDeclaration::Standard(StandardDeclaration { enum_spec, r#type, name, array_size: None }) => {
                    current_bitfield = None;
                    (name.clone(), Self::read_single_standard(enum_spec, r#type, read, parsed)?)
                },
                StructDeclaration::Standard(StandardDeclaration { enum_spec, r#type, name, array_size: Some(array_size) }) => {
                    current_bitfield = None;
                    let mut array = Vec::with_capacity(*array_size as usize);
                    for _ in 0..*array_size {
                        array.push(Self::read_single_standard(enum_spec, r#type, read, parsed)?);
                    }
                    (name.clone(), StructValue::Array(array))
                },
                StructDeclaration::Bitfield(BitfieldDeclaration { enum_spec, r#type, name, bits }) => {
                    let field_width = r#type.width().expect("bitfield has an integer type");
                    let (width, remaining_bits, data) = match &mut current_bitfield {
                        Some(bitfield) if field_width <= bitfield.0 && *bits <= bitfield.1 => {
                            bitfield
                        },
                        _ => {
                            let value = match r#type {
                                TypeName::Bool | TypeName::I8 | TypeName::U8 => read.read_i8()? as u64,
                                TypeName::I16 | TypeName::U16 => read.read_i16()? as u64,
                                TypeName::I32 | TypeName::U32 => read.read_i32()? as u64,
                                TypeName::I64 | TypeName::U64 => read.read_i64()? as u64,
                                _ => panic!("expected bitfield to have an integer type"),
                            };
                            current_bitfield.insert((field_width, field_width, value))
                        },
                    };

                    let mut value = 0u64;

                    let offset = 8 * ((*width - *remaining_bits) / 8) + (*width - *remaining_bits) % 8;
                    let mut mask = 1 << offset;

                    for _ in 0..*bits {
                        value |= mask & *data;
                        *remaining_bits -= 1;
                        mask <<= 1;
                    }
                    value >>= offset;
                    let value = match r#type {
                        TypeName::Bool => {
                            if value == 1 {
                                StructValue::Bool(true)
                            } else if value == 0 {
                                StructValue::Bool(false)
                            } else {
                                panic!("bitfield boolean is not 0 or 1");
                            }
                        },
                        TypeName::I8 => StructValue::I8(value as i8),
                        TypeName::I16 => StructValue::I16(value as i16),
                        TypeName::I32 => StructValue::I32(value as i32),
                        TypeName::I64 => StructValue::I64(value as i64),
                        TypeName::U8 => StructValue::U8(value as u8),
                        TypeName::U16 => StructValue::U16(value as u16),
                        TypeName::U32 => StructValue::U32(value as u32),
                        TypeName::U64 => StructValue::U64(value),
                        _ => panic!("expected bitfield to have integer type"),
                    };

                    if let Some(enum_spec) = enum_spec {
                        let enum_value = Self::find_enum_value(enum_spec, value)?;
                        (name.clone(), StructValue::Enum(enum_value.0, enum_value.1))
                    } else {
                        (name.clone(), value)
                    }
                },
            };

            fields.push(field);
        }

        Some(fields)
    }

    fn read_single_standard(enum_spec: &Option<EnumSpecification>, type_name: &TypeName, read: &mut ByteReader, parsed: &HashMap<String, ParsedStruct>) -> Option<StructValue> {
        let value = Self::read_type(type_name, read, parsed)?;
        if let Some(enum_spec) = enum_spec {
            let enum_value = Self::find_enum_value(enum_spec, value)?;
            Some(StructValue::Enum(enum_value.0, enum_value.1))
        } else {
            Some(value)
        }
    }

    fn find_enum_value(enum_spec: &EnumSpecification, value: StructValue) -> Option<(String, i32)> {
        for enum_value in &enum_spec.values {
            let value_int = match value {
                StructValue::I8(i8) => i8 as i32,
                StructValue::I16(i16) => i16 as i32,
                StructValue::I32(i32) => i32,
                StructValue::I64(i64) => i64 as i32,
                StructValue::U8(u8) => u8 as i32,
                StructValue::U16(u16) => u16 as i32,
                StructValue::U32(u32) => u32 as i32,
                StructValue::U64(u64) => u64 as i32,
                _ => panic!("non-int enum type"),
            };
            if value_int == enum_value.value {
                return Some((enum_value.name.clone(), value_int));
            }
        }
        None
    }

    fn read_type(type_name: &TypeName, read: &mut ByteReader, parsed: &HashMap<String, ParsedStruct>) -> Option<StructValue> {
        match type_name {
            TypeName::Bool => {
                let bool = read.read_i8()?;
                if bool == 1 {
                    Some(StructValue::Bool(true))
                } else if bool == 0 {
                    Some(StructValue::Bool(false))
                } else {
                    None
                }
            },
            TypeName::Char => Some(StructValue::Char(read.read_i8()? as u8 as char)),
            TypeName::I8 => Some(StructValue::I8(read.read_i8()?)),
            TypeName::I16 => Some(StructValue::I16(read.read_i16()?)),
            TypeName::I32 => Some(StructValue::I32(read.read_i32()?)),
            TypeName::I64 => Some(StructValue::I64(read.read_i64()?)),
            // TODO: maybe add unsigned read/write methods?
            TypeName::U8 => Some(StructValue::U8(read.read_i8()? as u8)),
            TypeName::U16 => Some(StructValue::U16(read.read_i16()? as u16)),
            TypeName::U32 => Some(StructValue::U32(read.read_i32()? as u32)),
            TypeName::U64 => Some(StructValue::U64(read.read_i64()? as u64)),
            TypeName::F32 => Some(StructValue::F32(read.read_f32()?)),
            TypeName::F64 => Some(StructValue::F64(read.read_f64()?)),
            TypeName::Struct(name) => {
                let parsed_struct = parsed.get(name)?;
                Some(StructValue::Nested(parsed_struct.read_from_bytes(read, parsed)?))
            },
        }
    }
}

/// Represents a value a struct field could have.
#[derive(Debug, Clone, PartialEq)]
pub enum StructValue {
    /// A boolean.
    Bool(bool),
    /// A UTF-8 character.
    Char(char),
    /// A 1-byte signed value.
    I8(i8),
    /// A 2-byte signed value.
    I16(i16),
    /// A 4-byte signed value.
    I32(i32),
    /// An 8-byte signed value.
    I64(i64),
    /// A 1-byte unsigned value.
    U8(u8),
    /// A 2-byte unsigned value.
    U16(u16),
    /// A 4-byte unsigned value.
    U32(u32),
    /// An 8-byte unsigned value.
    U64(u64),
    /// A 4-byte IEEE-754 value.
    F32(f32),
    /// An 8-byte IEEE-754 value.
    F64(f64),
    /// An array of values.
    ///
    /// All elements should have the same type.
    Array(Vec<StructValue>),
    /// An enum value, with its name and number representation.
    Enum(String, i32),
    /// A nested struct value.
    Nested(Vec<(String, StructValue)>),
}

/// A parsed struct declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StructDeclaration {
    /// A standard struct declaration.
    Standard(StandardDeclaration),
    /// A bitfield struct declaration.
    Bitfield(BitfieldDeclaration),
}

impl StructDeclaration {
    /// Parses a stream of tokens into a `StructDeclaration`, as described
    /// [here](https://github.com/wpilibsuite/allwpilib/blob/main/wpiutil/doc/struct.adoc#schema).
    pub fn parse_tokens(tokens: &mut TokenStream) -> Result<Self, ParseTokensError> {
        let enum_spec = match tokens.peek()? {
            Token::OpenBrace => Some(EnumSpecification::parse_tokens(tokens)?),
            Token::Ident(ident) if ident == "enum" => Some(EnumSpecification::parse_tokens(tokens)?),
            _ => None,
        };

        let type_name = TypeName::from_string(tokens.next_ident()?);

        if enum_spec.is_some() && !type_name.is_int() {
            return Err(ParseTokensError::Expected("enum to have an integer type".to_owned()));
        }

        let ident_name = tokens.next_ident()?;
        let array_size = if let Ok(Token::OpenBracket) = tokens.peek() {
            tokens.next_token()?;
            let size = tokens.next_number()?;
            if !size.is_positive() { return Err(ParseTokensError::Expected("array size to be positive".to_owned())); };
            if let Token::CloseBracket = tokens.next_token()? {
                Some(size as u32)
            } else {
                return Err(ParseTokensError::Expected("closed bracket".to_owned()))
            }
        } else {
            None
        };

        if array_size.is_some() {
            Ok(Self::Standard(StandardDeclaration { enum_spec, r#type: type_name, name: ident_name, array_size }))
        } else {
            match tokens.peek() {
                Ok(Token::Colon) => {
                    tokens.next_token()?;
                    let bits = tokens.next_number()?;
                    if !bits.is_positive() {
                        return Err(ParseTokensError::Expected("positive bitfield width".to_owned()));
                    }
                    let bits = bits as u32;
                    let max_width = if let TypeName::Bool = type_name {
                        1
                    } else {
                        type_name.width().ok_or(ParseTokensError::Expected("type to be a boolean or integer type".to_owned()))?
                    };
                    if bits <= max_width {
                        Ok(Self::Bitfield(BitfieldDeclaration { enum_spec, r#type: type_name, name: ident_name, bits }))
                    } else {
                        Err(ParseTokensError::Expected("number of bits to not be greater than maximum width of type".to_owned()))
                    }
                },
                _ => {
                    Ok(Self::Standard(StandardDeclaration { enum_spec, r#type: type_name, name: ident_name, array_size }))
                },
            }
        }
    }

    /// Returns the enum specification of this declaration, if any.
    pub fn enum_spec(&self) -> Option<&EnumSpecification> {
        match self {
            Self::Standard(StandardDeclaration { enum_spec, .. }) => enum_spec.as_ref(),
            Self::Bitfield(BitfieldDeclaration { enum_spec, .. }) => enum_spec.as_ref(),
        }
    }

    /// Returns the name of this declaration.
    pub fn name(&self) -> &String {
        match self {
            Self::Standard(StandardDeclaration { name, .. }) => name,
            Self::Bitfield(BitfieldDeclaration { name, .. }) => name,
        }
    }

    /// Returns the type of this declaration.
    pub fn type_name(&self) -> &TypeName {
        match self {
            Self::Standard(StandardDeclaration { r#type, .. }) => r#type,
            Self::Bitfield(BitfieldDeclaration { r#type, .. }) => r#type,
        }
    }
}

/// A standard struct declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardDeclaration {
    /// The enum specification, if any.
    pub enum_spec: Option<EnumSpecification>,
    /// The type.
    pub r#type: TypeName,
    /// The name.
    pub name: String,
    /// The array size, if any.
    pub array_size: Option<u32>,
}

/// A bitfield struct declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BitfieldDeclaration {
    /// The enum specification, if any.
    pub enum_spec: Option<EnumSpecification>,
    /// The type.
    pub r#type: TypeName,
    /// The name.
    pub name: String,
    /// The bits this declaration will take up in packed data.
    pub bits: u32,
}

/// An enum specification for a struct declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumSpecification {
    /// The enum values.
    pub values: Vec<EnumValue>,
}

impl EnumSpecification {
    /// Parses a stream of tokens into an `EnumSpecification`, as described [here](https://github.com/wpilibsuite/allwpilib/blob/main/wpiutil/doc/struct.adoc#enum-specification).
    pub fn parse_tokens(tokens: &mut TokenStream) -> Result<Self, ParseTokensError> {
        if let Ok(ident) = tokens.peek_ident() {
            if ident == "enum" {
                tokens.next_token()?;
            } else {
                return Err(ParseTokensError::Expected("`enum`".to_owned()));
            }
        }

        match tokens.next_token()? {
            Token::OpenBrace => {},
            _ => return Err(ParseTokensError::Expected("`{`".to_owned())),
        }

        let mut values = Vec::new();
        if let Token::CloseBrace = tokens.peek()? {
            tokens.next_token()?;
            return Ok(Self { values });
        };
        values.push(EnumValue::parse_tokens(tokens)?);

        loop {
            match tokens.next_token()? {
                Token::CloseBrace => break,
                Token::Comma => {},
                _ => return Err(ParseTokensError::Expected("`,` or `}`".to_owned())),
            }
            if let Token::CloseBrace = tokens.peek()? {
                tokens.next_token()?;
                break;
            }

            values.push(EnumValue::parse_tokens(tokens)?);
        }

        Ok(Self { values })
    }
}

/// A value for an enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumValue {
    /// The name of the variant.
    pub name: String,
    /// The value of the variant.
    pub value: i32,
}

impl EnumValue {
    /// Parses a stream of tokens into an `EnumValue`.
    pub fn parse_tokens(tokens: &mut TokenStream) -> Result<Self, ParseTokensError> {
        let name = tokens.next_ident()?;
        match tokens.next_token()? {
            Token::Eq => {},
            _ => return Err(ParseTokensError::Expected("`=`".to_owned())),
        }
        let value = tokens.next_number()?;
        Ok(Self { name, value })
    }
}

/// The type a struct member can be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeName {
    /// A boolean.
    Bool,
    /// A UTF-8 character.
    Char,
    /// A 1-byte signed value.
    I8,
    /// A 2-byte signed value.
    I16,
    /// A 4-byte signed value.
    I32,
    /// An 8-byte signed value.
    I64,
    /// A 1-byte unsigned value.
    U8,
    /// A 2-byte unsigned value.
    U16,
    /// A 4-byte unsigned value.
    U32,
    /// An 8-byte unsigned value.
    U64,
    /// A 4-byte IEEE-754 value.
    F32,
    /// An 8-byte IEEE-754 value.
    F64,
    /// A nested struct value.
    Struct(String),
}

impl TypeName {
    /// Returns a type from a string.
    pub fn from_string(str: String) -> Self {
        match str.as_ref() {
            "bool" => Self::Bool,
            "char" => Self::Char,
            "int8" => Self::I8,
            "int16" => Self::I16,
            "int32" => Self::I32,
            "int64" => Self::I64,
            "uint8" => Self::U8,
            "uint16" => Self::U16,
            "uint32" => Self::U32,
            "uint64" => Self::U64,
            "float" | "float32" => Self::F32,
            "double" | "float64" => Self::F64,
            _ => Self::Struct(str),
        }
    }

    /// Returns if this type is an integer type or not.
    pub fn is_int(&self) -> bool {
        !matches!(self, TypeName::Bool | TypeName::Char | TypeName::F32 | TypeName::F64 | TypeName::Struct(_))
    }

    /// Returns the width of this type, if it is an integer type or a boolean.
    pub fn width(&self) -> Option<u32> {
        match self {
            TypeName::Bool => Some(i8::BITS),
            TypeName::I8 => Some(i8::BITS),
            TypeName::I16 => Some(i16::BITS),
            TypeName::I32 => Some(i32::BITS),
            TypeName::I64 => Some(i64::BITS),
            TypeName::U8 => Some(u8::BITS),
            TypeName::U16 => Some(u16::BITS),
            TypeName::U32 => Some(u32::BITS),
            TypeName::U64 => Some(u64::BITS),
            _ => None,
        }
    }
}

/// Errors that can occur when lexing a schema.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum ParseTokensError {
    /// Expected something else.
    #[error("Expected {0}")]
    Expected(String),

    /// Unexpected EOF.
    #[error("Unexpected EOF")]
    Eof,
}

/// Errors that can occur when parsing a [`ParsedStruct`].
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum ParseSchemaError {
    /// A lexing error.
    #[error(transparent)]
    Lex(#[from] LexTokenError),

    /// A parsing error.
    #[error(transparent)]
    Parse(#[from] ParseTokensError),
}

/// Parses a schema string into a [`ParsedStruct`], as described
/// [here](https://github.com/wpilibsuite/allwpilib/blob/main/wpiutil/doc/struct.adoc).
pub fn parse_schema(schema: &str) -> Result<ParsedStruct, ParseSchemaError> {
    let mut stream = lex(schema)?;
    Ok(ParsedStruct::parse_tokens(&mut stream)?)
}

/// A token from a schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    /// A semicolon, `;`.
    Semi,
    /// A comma, `,`.
    Comma,
    /// A colon, `:`.
    Colon,
    /// An equal sign, `=`.
    Eq,
    /// An open brace, `{`.
    OpenBrace,
    /// A close brace, `}`.
    CloseBrace,
    /// An open bracket, `[`.
    OpenBracket,
    /// A close bracket, `]`.
    CloseBracket,
    /// An identifier.
    Ident(String),
    /// A number.
    Number(i32),
}

/// Errors that can occur when lexing a schema.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum LexTokenError {
    /// An invalid digit.
    #[error("Invalid digit `{0}`")]
    InvalidDigit(char),

    /// An invalid character.
    #[error("Invalid token `{0}`")]
    InvalidToken(char),
}

/// Lexes a schema input into a stream of tokens.
pub fn lex(input: &str) -> Result<TokenStream, LexTokenError> {
    let mut tokens = Vec::new();

    let mut chars = input.chars().peekable();
    while let Some(char) = chars.peek() {
        match char {
            ';' => tokens.push(Token::Semi),
            ',' => tokens.push(Token::Comma),
            ':' => tokens.push(Token::Colon),
            '=' => tokens.push(Token::Eq),
            '{' => tokens.push(Token::OpenBrace),
            '}' => tokens.push(Token::CloseBrace),
            '[' => tokens.push(Token::OpenBracket),
            ']' => tokens.push(Token::CloseBracket),
            '-' => tokens.push(Token::Number(-read_int(&mut chars)?)),
            _ if char.is_whitespace() => { },
            _ => {
                if char.is_ascii_digit() {
                    tokens.push(Token::Number(read_int(&mut chars)?));
                } else if char.is_alphabetic() {
                    tokens.push(Token::Ident(read_ident(&mut chars)?));
                } else {
                    return Err(LexTokenError::InvalidToken(*char));
                }
                continue;
            },
        }
        chars.next();
    }

    Ok(TokenStream::new(tokens))
}

fn read_int(chars: &mut Peekable<Chars>) -> Result<i32, LexTokenError> {
    let mut curr = 0;
    while let Some(char) = chars.peek() {
        if char.is_whitespace() { break; };

        if let Some(digit) = char.to_digit(10) {
            curr *= 10;
            curr += digit as i32;
            chars.next();
        } else {
            break;
        }
    }

    Ok(curr)
}

fn read_ident(chars: &mut Peekable<Chars>) -> Result<String, LexTokenError> {
    let mut ident = String::new();
    while let Some(char) = chars.peek() {
        if char.is_whitespace() { break; };
        if char.is_alphanumeric() {
            ident += &char.to_string();
            chars.next();
        } else {
            break;
        }
    }

    Ok(ident)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lexer() {
        assert_eq!(lex("bool value"), Ok(TokenStream::new(vec![Token::Ident("bool".to_owned()), Token::Ident("value".to_owned())])));
        assert_eq!(lex("double array[4]"), Ok(TokenStream::new(vec![
            Token::Ident("double".to_owned()),
            Token::Ident("array".to_owned()),
            Token::OpenBracket,
            Token::Number(4),
            Token::CloseBracket,
        ])));
        assert_eq!(lex("enum {a=1, b=2} int8 val"), Ok(TokenStream::new(vec![
            Token::Ident("enum".to_owned()),
            Token::OpenBrace,
            Token::Ident("a".to_owned()),
            Token::Eq,
            Token::Number(1),
            Token::Comma,
            Token::Ident("b".to_owned()),
            Token::Eq,
            Token::Number(2),
            Token::CloseBrace,
            Token::Ident("int8".to_owned()),
            Token::Ident("val".to_owned()),
        ])));
    }

    #[test]
    fn test_parser() {
        use StructDeclaration as S;

        assert_eq!(parse_schema("bool value"), Ok(ParsedStruct { declarations: vec![
            S::Standard(StandardDeclaration { enum_spec: None, r#type: TypeName::Bool, name: "value".to_owned(), array_size: None }),
        ], deps: Vec::new() }));

        assert_eq!(parse_schema("double array[4]"), Ok(ParsedStruct { declarations: vec![
            S::Standard(StandardDeclaration { enum_spec: None, r#type: TypeName::F64, name: "array".to_owned(), array_size: Some(4) }),
        ], deps: Vec::new() }));

        assert_eq!(parse_schema("enum {a=1, b=2} int8 val"), Ok(ParsedStruct { declarations: vec![
            S::Standard(StandardDeclaration {
                enum_spec: Some(EnumSpecification { values: vec![EnumValue { name: "a".to_owned(), value: 1 }, EnumValue { name: "b".to_owned(), value: 2 }] }),
                r#type: TypeName::I8,
                name: "val".to_owned(),
                array_size: None,
            }),
        ], deps: Vec::new() }));

        assert_eq!(parse_schema("enum {a=1,b=2,} int8 val"), Ok(ParsedStruct { declarations: vec![
            S::Standard(StandardDeclaration {
                enum_spec: Some(EnumSpecification { values: vec![EnumValue { name: "a".to_owned(), value: 1 }, EnumValue { name: "b".to_owned(), value: 2 }] }),
                r#type: TypeName::I8,
                name: "val".to_owned(),
                array_size: None,
            }),
        ], deps: Vec::new() }));

        assert_eq!(parse_schema("uint16 value:5"), Ok(ParsedStruct { declarations: vec![
            S::Bitfield(BitfieldDeclaration {
                enum_spec: None,
                r#type: TypeName::U16,
                name: "value".to_owned(),
                bits: 5,
            }),
        ], deps: Vec::new() }));

        assert_eq!(parse_schema("uint16 value:5;{a=1,b=2} uint16 other;;bool flag;;;"), Ok(ParsedStruct { declarations: vec![
            S::Bitfield(BitfieldDeclaration {
                enum_spec: None,
                r#type: TypeName::U16,
                name: "value".to_owned(),
                bits: 5,
            }),
            S::Standard(StandardDeclaration {
                enum_spec: Some(EnumSpecification { values: vec![EnumValue { name: "a".to_owned(), value: 1 }, EnumValue { name: "b".to_owned(), value: 2 }] }),
                r#type: TypeName::U16,
                name: "other".to_owned(),
                array_size: None,
            }),
            S::Standard(StandardDeclaration {
                enum_spec: None,
                r#type: TypeName::Bool,
                name: "flag".to_owned(),
                array_size: None,
            }),
        ], deps: Vec::new() }));
    }

    #[test]
    fn test_read() {
        assert_read("bool b; int16 i", &[0x01, 0x0f, 0x00], vec![
            ("b", StructValue::Bool(true)),
            ("i", StructValue::I16(15)),
        ]);
        assert_read("int16 i[2]", &[0x95, 0x03, 0xda, 0xff], vec![("i", StructValue::Array(vec![StructValue::I16(917), StructValue::I16(-38)]))]);
        assert_read("{a=1, b=2} uint8 myenum", &[0x02], vec![("myenum", StructValue::Enum("b".to_owned(), 2))]);
        assert_read("int8 a:4;int16 b:4", &[
//            0000_aaaa    0000_bbbb    0000_0000
            0b0000_0110, 0b0000_1101, 0b0000_0000,
        ], vec![("a", StructValue::I8(6)), ("b", StructValue::I16(13))]);
        assert_read("int16 a:4;uint16 b:5;bool c:1;int16 d:7", &[
//            bbbb_aaaa    0000_00cb    0ddd_dddd    0000_0000
            0b0111_1010, 0b0000_0010, 0b0011_0101, 0b0000_0000,
        ], vec![("a", StructValue::I16(10)), ("b", StructValue::U16(7)), ("c", StructValue::Bool(true)), ("d", StructValue::I16(53))]);
        assert_read("uint8 a:4;int8 b:2;bool c:1;int16 d:1", &[
//            0cbb_aaaa    0000_000d    0000_0000
            0b0001_0110, 0b0000_0001, 0b0000_0000,
        ], vec![("a", StructValue::U8(6)), ("b", StructValue::I8(1)), ("c", StructValue::Bool(false)), ("d", StructValue::I16(1))]);
        assert_read("bool a:1;bool b:1;int8 c:2", &[
//            0000_ccba
            0b0000_1001,
        ], vec![("a", StructValue::Bool(true)), ("b", StructValue::Bool(false)), ("c", StructValue::I8(2))]);
        assert_read("bool a:1;bool b:1;int16 c:2", &[
//            0000_00ba    0000_00cc    0000_0000
            0b0000_0101, 0b0000_0001, 0b0000_0000,
        ], vec![("a", StructValue::Bool(true)), ("b", StructValue::Bool(false)), ("c", StructValue::I16(1))]);
        assert_read("enum {a=1,b=2} int16 a:2;bool b:1", &[
//            0000_0baa    0000_0000
            0b0000_0101, 0b0000_0000,
        ], vec![("a", StructValue::Enum("a".to_owned(), 1)), ("b", StructValue::Bool(true))]);

        let mut parsed = HashMap::new();
        parsed.insert("Coords".to_owned(), parse_schema("double x;double y").unwrap());
        parsed.insert("Flags".to_owned(), parse_schema("bool a:1;bool b:1;bool c:1").unwrap());

        let bytes = [
            0xf6, 0x07,
            0x5c, 0x8f, 0xc2, 0xf5, 0x28, 0x1c, 0x4d, 0xc0,
            0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x10, 0x40,
            0b0000_0101,
        ];
        assert_eq!(
            parse_schema("uint16 id;Coords coords;Flags flags").unwrap().read_from_bytes(&mut ByteReader::new(&bytes), &parsed).unwrap(),
            vec![
                ("id".to_owned(), StructValue::U16(2038)),
                ("coords".to_owned(), StructValue::Nested(vec![("x".to_owned(), StructValue::F64(-58.22)), ("y".to_owned(), StructValue::F64(4.1))])),
                ("flags".to_owned(), StructValue::Nested(vec![("a".to_owned(), StructValue::Bool(true)), ("b".to_owned(), StructValue::Bool(false)), ("c".to_owned(), StructValue::Bool(true))])),
            ],
        );
    }

    fn assert_read(schema: &str, bytes: &[u8], values: Vec<(&str, StructValue)>) {
        let parsed = parse_schema(schema).unwrap();
        let values = values.into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect();

        assert_eq!(parsed.read_from_bytes(&mut ByteReader::new(bytes), &Default::default()), Some(values));
    }
}

