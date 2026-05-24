use bytes::{Buf, BufMut, Bytes, BytesMut};
use std::any::TypeId;
use std::str;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum XdrError {
    #[error("unexpected end of input while reading {needed} bytes")]
    UnexpectedEof { needed: usize },
    #[error("invalid boolean discriminator: {0}")]
    InvalidBool(u32),
    #[error("invalid enum discriminant: {0}")]
    InvalidEnum(i32),
    #[error("invalid UTF-8 string: {0}")]
    InvalidUtf8(String),
    #[error("trailing bytes after XDR decode: {0}")]
    TrailingBytes(usize),
}

pub trait XdrEncode {
    fn encode_xdr(&self, output: &mut BytesMut) -> Result<(), XdrError>;

    fn to_xdr_bytes(&self) -> Result<Bytes, XdrError> {
        let mut output = BytesMut::new();
        self.encode_xdr(&mut output)?;
        Ok(output.freeze())
    }
}

pub trait XdrDecode: Sized {
    fn decode_xdr(input: &mut &[u8]) -> Result<Self, XdrError>;

    fn from_xdr_bytes(input: &[u8]) -> Result<Self, XdrError> {
        let mut input = input;
        let value = Self::decode_xdr(&mut input)?;
        if !input.is_empty() {
            return Err(XdrError::TrailingBytes(input.len()));
        }
        Ok(value)
    }
}

impl XdrEncode for bool {
    fn encode_xdr(&self, output: &mut BytesMut) -> Result<(), XdrError> {
        output.put_u32(u32::from(*self));
        Ok(())
    }
}

impl XdrDecode for bool {
    fn decode_xdr(input: &mut &[u8]) -> Result<Self, XdrError> {
        match read_u32(input)? {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(XdrError::InvalidBool(other)),
        }
    }
}

impl XdrEncode for i32 {
    fn encode_xdr(&self, output: &mut BytesMut) -> Result<(), XdrError> {
        output.put_i32(*self);
        Ok(())
    }
}

impl XdrDecode for i32 {
    fn decode_xdr(input: &mut &[u8]) -> Result<Self, XdrError> {
        read_i32(input)
    }
}

impl XdrEncode for u32 {
    fn encode_xdr(&self, output: &mut BytesMut) -> Result<(), XdrError> {
        output.put_u32(*self);
        Ok(())
    }
}

impl XdrDecode for u32 {
    fn decode_xdr(input: &mut &[u8]) -> Result<Self, XdrError> {
        read_u32(input)
    }
}

impl XdrEncode for i64 {
    fn encode_xdr(&self, output: &mut BytesMut) -> Result<(), XdrError> {
        output.put_i64(*self);
        Ok(())
    }
}

impl XdrDecode for i64 {
    fn decode_xdr(input: &mut &[u8]) -> Result<Self, XdrError> {
        read_i64(input)
    }
}

impl XdrEncode for u64 {
    fn encode_xdr(&self, output: &mut BytesMut) -> Result<(), XdrError> {
        output.put_u64(*self);
        Ok(())
    }
}

impl XdrDecode for u64 {
    fn decode_xdr(input: &mut &[u8]) -> Result<Self, XdrError> {
        read_u64(input)
    }
}

impl XdrEncode for f32 {
    fn encode_xdr(&self, output: &mut BytesMut) -> Result<(), XdrError> {
        output.put_u32(self.to_bits());
        Ok(())
    }
}

impl XdrDecode for f32 {
    fn decode_xdr(input: &mut &[u8]) -> Result<Self, XdrError> {
        Ok(f32::from_bits(read_u32(input)?))
    }
}

impl XdrEncode for f64 {
    fn encode_xdr(&self, output: &mut BytesMut) -> Result<(), XdrError> {
        output.put_u64(self.to_bits());
        Ok(())
    }
}

impl XdrDecode for f64 {
    fn decode_xdr(input: &mut &[u8]) -> Result<Self, XdrError> {
        Ok(f64::from_bits(read_u64(input)?))
    }
}

impl XdrEncode for u8 {
    fn encode_xdr(&self, output: &mut BytesMut) -> Result<(), XdrError> {
        output.put_u8(*self);
        Ok(())
    }
}

impl XdrDecode for u8 {
    fn decode_xdr(input: &mut &[u8]) -> Result<Self, XdrError> {
        ensure_len(input, 1)?;
        Ok(input.get_u8())
    }
}

impl XdrEncode for String {
    fn encode_xdr(&self, output: &mut BytesMut) -> Result<(), XdrError> {
        write_variable_opaque(output, self.as_bytes())
    }
}

impl XdrDecode for String {
    fn decode_xdr(input: &mut &[u8]) -> Result<Self, XdrError> {
        let bytes = read_variable_opaque(input)?;
        let value =
            str::from_utf8(&bytes).map_err(|error| XdrError::InvalidUtf8(error.to_string()))?;
        Ok(value.to_string())
    }
}

impl XdrEncode for Bytes {
    fn encode_xdr(&self, output: &mut BytesMut) -> Result<(), XdrError> {
        write_variable_opaque(output, self)
    }
}

impl XdrDecode for Bytes {
    fn decode_xdr(input: &mut &[u8]) -> Result<Self, XdrError> {
        read_variable_opaque(input)
    }
}

impl<T> XdrEncode for Vec<T>
where
    T: XdrEncode,
{
    fn encode_xdr(&self, output: &mut BytesMut) -> Result<(), XdrError> {
        output.put_u32(self.len() as u32);
        for value in self {
            value.encode_xdr(output)?;
        }
        Ok(())
    }
}

impl<T> XdrDecode for Vec<T>
where
    T: XdrDecode,
{
    fn decode_xdr(input: &mut &[u8]) -> Result<Self, XdrError> {
        let len = read_u32(input)? as usize;
        let mut values = Vec::with_capacity(len);
        for _ in 0..len {
            values.push(T::decode_xdr(input)?);
        }
        Ok(values)
    }
}

impl<T> XdrEncode for Option<Box<T>>
where
    T: XdrEncode,
{
    fn encode_xdr(&self, output: &mut BytesMut) -> Result<(), XdrError> {
        match self {
            Some(value) => {
                true.encode_xdr(output)?;
                value.encode_xdr(output)
            }
            None => false.encode_xdr(output),
        }
    }
}

impl<T> XdrDecode for Option<Box<T>>
where
    T: XdrDecode,
{
    fn decode_xdr(input: &mut &[u8]) -> Result<Self, XdrError> {
        if bool::decode_xdr(input)? {
            Ok(Some(Box::new(T::decode_xdr(input)?)))
        } else {
            Ok(None)
        }
    }
}

impl<T, const N: usize> XdrEncode for [T; N]
where
    T: XdrEncode + XdrDecode + 'static,
{
    fn encode_xdr(&self, output: &mut BytesMut) -> Result<(), XdrError> {
        if TypeId::of::<T>() == TypeId::of::<u8>() {
            let bytes = self
                .iter()
                .map(|value| {
                    let mut out = BytesMut::new();
                    value.encode_xdr(&mut out)?;
                    Ok(out[0])
                })
                .collect::<Result<Vec<_>, XdrError>>()?;
            write_fixed_opaque(output, &bytes);
        } else {
            for value in self {
                value.encode_xdr(output)?;
            }
        }
        Ok(())
    }
}

impl<T, const N: usize> XdrDecode for [T; N]
where
    T: XdrEncode + XdrDecode + 'static,
{
    fn decode_xdr(input: &mut &[u8]) -> Result<Self, XdrError> {
        if TypeId::of::<T>() == TypeId::of::<u8>() {
            let bytes = read_fixed_opaque(input, N)?;
            let mut values = Vec::with_capacity(N);
            for index in 0..N {
                let mut element = &bytes[index..index + 1];
                values.push(T::decode_xdr(&mut element)?);
            }
            Ok(values
                .try_into()
                .unwrap_or_else(|_| unreachable!("fixed array length must match")))
        } else {
            decode_fixed_array(input)
        }
    }
}

pub fn write_fixed_opaque(output: &mut BytesMut, bytes: &[u8]) {
    output.extend_from_slice(bytes);
    pad_to_xdr_alignment(output, bytes.len());
}

pub fn read_fixed_opaque(input: &mut &[u8], len: usize) -> Result<Bytes, XdrError> {
    let bytes = read_exact(input, len)?;
    discard_padding(input, len)?;
    Ok(bytes)
}

pub fn write_variable_opaque(output: &mut BytesMut, bytes: &[u8]) -> Result<(), XdrError> {
    output.put_u32(bytes.len() as u32);
    write_fixed_opaque(output, bytes);
    Ok(())
}

pub fn read_variable_opaque(input: &mut &[u8]) -> Result<Bytes, XdrError> {
    let len = read_u32(input)? as usize;
    read_fixed_opaque(input, len)
}

pub fn read_u32(input: &mut &[u8]) -> Result<u32, XdrError> {
    ensure_len(input, 4)?;
    Ok(input.get_u32())
}

pub fn read_i32(input: &mut &[u8]) -> Result<i32, XdrError> {
    ensure_len(input, 4)?;
    Ok(input.get_i32())
}

pub fn read_u64(input: &mut &[u8]) -> Result<u64, XdrError> {
    ensure_len(input, 8)?;
    Ok(input.get_u64())
}

pub fn read_i64(input: &mut &[u8]) -> Result<i64, XdrError> {
    ensure_len(input, 8)?;
    Ok(input.get_i64())
}

pub fn read_exact(input: &mut &[u8], len: usize) -> Result<Bytes, XdrError> {
    ensure_len(input, len)?;
    Ok(input.copy_to_bytes(len))
}

pub fn decode_fixed_array<T, const N: usize>(input: &mut &[u8]) -> Result<[T; N], XdrError>
where
    T: XdrDecode,
{
    let mut values = Vec::with_capacity(N);
    for _ in 0..N {
        values.push(T::decode_xdr(input)?);
    }
    Ok(values
        .try_into()
        .unwrap_or_else(|_| unreachable!("fixed array length must match")))
}

fn ensure_len(input: &[u8], len: usize) -> Result<(), XdrError> {
    if input.len() < len {
        Err(XdrError::UnexpectedEof { needed: len })
    } else {
        Ok(())
    }
}

fn pad_to_xdr_alignment(output: &mut BytesMut, len: usize) {
    let padding = xdr_padding(len);
    output.resize(output.len() + padding, 0);
}

fn discard_padding(input: &mut &[u8], len: usize) -> Result<(), XdrError> {
    let padding = xdr_padding(len);
    ensure_len(input, padding)?;
    *input = &input[padding..];
    Ok(())
}

fn xdr_padding(len: usize) -> usize {
    (4 - (len % 4)) % 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_round_trips() {
        let value = "abc".to_string();
        let bytes = value.to_xdr_bytes().expect("encode");
        assert_eq!(&bytes[..], &[0, 0, 0, 3, b'a', b'b', b'c', 0]);
        let decoded = String::from_xdr_bytes(&bytes).expect("decode");
        assert_eq!(decoded, value);
    }

    #[test]
    fn bytes_round_trip() {
        let value = Bytes::from_static(b"hello");
        let decoded =
            Bytes::from_xdr_bytes(&value.to_xdr_bytes().expect("encode")).expect("decode");
        assert_eq!(decoded, value);
    }

    #[test]
    fn optional_box_round_trip() {
        let value = Some(Box::new(7_u32));
        let decoded = Option::<Box<u32>>::from_xdr_bytes(&value.to_xdr_bytes().expect("encode"))
            .expect("decode");
        assert_eq!(decoded, value);
    }

    #[test]
    fn fixed_opaque_round_trip() {
        let value = [1_u8, 2, 3];
        let bytes = value.to_xdr_bytes().expect("encode");
        assert_eq!(&bytes[..], &[1, 2, 3, 0]);
        let decoded = <[u8; 3]>::from_xdr_bytes(&bytes).expect("decode");
        assert_eq!(decoded, value);
    }

    #[test]
    fn invalid_bool_fails() {
        let error = bool::from_xdr_bytes(&[0, 0, 0, 2]).expect_err("invalid bool");
        assert_eq!(error, XdrError::InvalidBool(2));
    }
}
