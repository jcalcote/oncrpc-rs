//! Segmented XDR encoding and metadata-only decoding for caller-owned bodies.
use crate::{XdrDecode, XdrEncode, XdrError};
use bytes::{Bytes, BytesMut};

/// Encode control fields without coalescing immutable opaque owners.
pub trait XdrEncodeSegments {
    /// Append this value's complete XDR representation, including padding.
    fn encode_xdr_segments(&self, output: &mut SegmentEncoder) -> Result<(), XdrError>;

    /// Produce owned segments. Opaque bodies retain their original backing.
    fn to_xdr_segments(&self) -> Result<Vec<Bytes>, XdrError> {
        let mut output = SegmentEncoder::default();
        self.encode_xdr_segments(&mut output)?;
        Ok(output.finish())
    }
}

/// Groups adjacent control fields and retains opaque bodies as separate owners.
#[derive(Default)]
pub struct SegmentEncoder {
    control: BytesMut,
    segments: Vec<Bytes>,
}
impl SegmentEncoder {
    /// Encode a control value into the current small contiguous segment.
    pub fn encode<T: XdrEncode + ?Sized>(&mut self, value: &T) -> Result<(), XdrError> {
        value.encode_xdr(&mut self.control)
    }

    /// Append a variable opaque value, retaining its original immutable owner.
    ///
    /// `maximum` is the IDL bound, when declared. Length and padding remain XDR.
    pub fn opaque(&mut self, value: &Bytes, maximum: Option<usize>) -> Result<(), XdrError> {
        let maximum = maximum.unwrap_or(u32::MAX as usize).min(u32::MAX as usize);
        if value.len() > maximum {
            return Err(XdrError::LengthLimit {
                length: value.len(),
                maximum,
            });
        }
        self.encode(&(value.len() as u32))?;
        self.flush();
        if !value.is_empty() {
            self.segments.push(value.clone());
        }
        let padding = (4 - value.len() % 4) % 4;
        if padding != 0 {
            self.segments
                .push(Bytes::from_static(&[0; 3]).slice(..padding));
        }
        Ok(())
    }

    /// Append multiple owners as one XDR opaque body with one length and padding.
    pub fn opaque_segments<const MAX: usize>(
        &mut self,
        value: &OpaqueSegments<MAX>,
    ) -> Result<(), XdrError> {
        let length = value.checked_len()?;
        self.encode(&(length as u32))?;
        self.flush();
        self.segments.extend(
            value
                .segments
                .iter()
                .filter(|segment| !segment.is_empty())
                .cloned(),
        );
        let padding = (4 - length % 4) % 4;
        if padding != 0 {
            self.segments
                .push(Bytes::from_static(&[0; 3]).slice(..padding));
        }
        Ok(())
    }

    fn flush(&mut self) {
        if !self.control.is_empty() {
            self.segments.push(self.control.split().freeze());
        }
    }

    /// Finish encoding; no payload segments are joined or copied.
    pub fn finish(mut self) -> Vec<Bytes> {
        self.flush();
        self.segments
    }
}

/// Immutable owners comprising one variable opaque XDR value.
///
/// Segment boundaries do not add XDR lengths or padding. The declared bound and
/// the protocol's 32-bit length limit are checked before encoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpaqueSegments<const MAX: usize = { u32::MAX as usize }> {
    segments: Vec<Bytes>,
}
impl<const MAX: usize> OpaqueSegments<MAX> {
    /// Retain owners after validating their combined wire length.
    pub fn try_new(segments: Vec<Bytes>) -> Result<Self, XdrError> {
        let value = Self { segments };
        value.checked_len()?;
        Ok(value)
    }
    /// Retained immutable owners, in wire order.
    pub fn segments(&self) -> &[Bytes] {
        &self.segments
    }
    /// Return the original immutable owners.
    pub fn into_segments(self) -> Vec<Bytes> {
        self.segments
    }

    fn checked_len(&self) -> Result<usize, XdrError> {
        let maximum = MAX.min(u32::MAX as usize);
        self.segments.iter().try_fold(0usize, |length, segment| {
            let length = length
                .checked_add(segment.len())
                .ok_or(XdrError::LengthLimit {
                    length: usize::MAX,
                    maximum,
                })?;
            if length > maximum {
                return Err(XdrError::LengthLimit { length, maximum });
            }
            Ok(length)
        })
    }
}
impl<const MAX: usize> From<Bytes> for OpaqueSegments<MAX> {
    fn from(value: Bytes) -> Self {
        Self {
            segments: vec![value],
        }
    }
}
impl<const MAX: usize> XdrEncodeSegments for OpaqueSegments<MAX> {
    fn encode_xdr_segments(&self, output: &mut SegmentEncoder) -> Result<(), XdrError> {
        output.opaque_segments(self)
    }
}
impl<const MAX: usize> XdrEncode for OpaqueSegments<MAX> {
    fn encode_xdr(&self, output: &mut BytesMut) -> Result<(), XdrError> {
        let length = self.checked_len()?;
        (length as u32).encode_xdr(output)?;
        for segment in &self.segments {
            output.extend_from_slice(segment);
        }
        output.extend_from_slice(&[0; 3][..(4 - length % 4) % 4]);
        Ok(())
    }
}
impl<const MAX: usize> XdrDecode for OpaqueSegments<MAX> {
    fn decode_xdr(input: &mut &[u8]) -> Result<Self, XdrError> {
        // Check the bound before the ordinary decoder allocates its body.
        let mut prefix = *input;
        OpaqueLength::<MAX>::decode_xdr(&mut prefix)?;
        Bytes::decode_xdr(input).map(Self::from)
    }
}

macro_rules! control_encoders {
    ($($ty:ty),* $(,)?) => {$(
        impl XdrEncodeSegments for $ty {
            fn encode_xdr_segments(&self, output: &mut SegmentEncoder) -> Result<(), XdrError> {
                output.encode(self)
            }
        }
    )*};
}
control_encoders!((), bool, i32, u32, i64, u64, f32, f64, u8, String);

impl XdrEncodeSegments for Bytes {
    fn encode_xdr_segments(&self, output: &mut SegmentEncoder) -> Result<(), XdrError> {
        output.opaque(self, None)
    }
}
impl<T: XdrEncodeSegments + ?Sized> XdrEncodeSegments for &T {
    fn encode_xdr_segments(&self, output: &mut SegmentEncoder) -> Result<(), XdrError> {
        (*self).encode_xdr_segments(output)
    }
}
impl<T: XdrEncodeSegments> XdrEncodeSegments for Vec<T> {
    fn encode_xdr_segments(&self, output: &mut SegmentEncoder) -> Result<(), XdrError> {
        let length = u32::try_from(self.len()).map_err(|_| XdrError::LengthLimit {
            length: self.len(),
            maximum: u32::MAX as usize,
        })?;
        output.encode(&length)?;
        for value in self {
            value.encode_xdr_segments(output)?;
        }
        Ok(())
    }
}
impl<T: XdrEncodeSegments> XdrEncodeSegments for Option<Box<T>> {
    fn encode_xdr_segments(&self, output: &mut SegmentEncoder) -> Result<(), XdrError> {
        output.encode(&self.is_some())?;
        if let Some(value) = self {
            value.encode_xdr_segments(output)?;
        }
        Ok(())
    }
}
// Arrays retain the existing codec's fixed-opaque treatment of [u8; N].
impl<T: XdrEncodeSegments + XdrEncode + XdrDecode + 'static, const N: usize> XdrEncodeSegments
    for [T; N]
{
    fn encode_xdr_segments(&self, output: &mut SegmentEncoder) -> Result<(), XdrError> {
        if std::any::TypeId::of::<T>() == std::any::TypeId::of::<u8>() {
            output.encode(self)
        } else {
            for value in self {
                value.encode_xdr_segments(output)?;
            }
            Ok(())
        }
    }
}

/// Metadata projection of a result with at most one terminal opaque body.
///
/// Decoding consumes metadata and the body's XDR length, never the body itself.
/// Generated implementations accept only statically bounded control layouts.
pub trait XdrReplyMetadata: XdrDecode + Send + 'static {
    /// Maximum prefix bytes, including the opaque length on body-bearing arms.
    const MAX_PREFIX_BYTES: usize;
    /// Body length, or `None` for an arm with no opaque body.
    fn opaque_len(&self) -> Option<usize>;
}

/// The validated length of an opaque body that remains in caller-owned storage.
///
/// This is a metadata field, not a replacement for a complete opaque XDR value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpaqueLength<const MAX: usize = { u32::MAX as usize }>(usize);
impl<const MAX: usize> OpaqueLength<MAX> {
    /// Length of the following body, excluding XDR padding.
    pub fn len(&self) -> usize {
        self.0
    }
    /// Whether the following body is empty.
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }
}
impl<const MAX: usize> XdrDecode for OpaqueLength<MAX> {
    fn decode_xdr(input: &mut &[u8]) -> Result<Self, XdrError> {
        let length = u32::decode_xdr(input)? as usize;
        if length > MAX {
            return Err(XdrError::LengthLimit {
                length,
                maximum: MAX,
            });
        }
        Ok(Self(length))
    }
}
impl<const MAX: usize> XdrReplyMetadata for OpaqueLength<MAX> {
    const MAX_PREFIX_BYTES: usize = 4;
    fn opaque_len(&self) -> Option<usize> {
        Some(self.0)
    }
}
impl<M: XdrReplyMetadata> XdrReplyMetadata for Option<Box<M>> {
    const MAX_PREFIX_BYTES: usize = 4 + M::MAX_PREFIX_BYTES;
    fn opaque_len(&self) -> Option<usize> {
        self.as_deref().and_then(XdrReplyMetadata::opaque_len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_segments_match_wire_and_retain_backing() {
        for size in [0, 1, 2, 3, 4, 65_537] {
            let data = Bytes::from_owner(vec![37; size]);
            let segments = data.to_xdr_segments().unwrap();
            let actual: Vec<u8> = segments.iter().flat_map(|s| s.iter().copied()).collect();
            assert_eq!(actual, data.to_xdr_bytes().unwrap());
            if size != 0 {
                assert_eq!(segments[1].as_ptr(), data.as_ptr());
            }
        }
    }

    #[test]
    fn metadata_length_rejects_excess_without_consuming_body() {
        let wire = Bytes::from_static(&[0, 0, 0, 3, 9, 8, 7, 0]);
        let mut input = &wire[..];
        assert_eq!(OpaqueLength::<3>::decode_xdr(&mut input).unwrap().len(), 3);
        assert_eq!(input, &[9, 8, 7, 0]);
        assert_eq!(
            OpaqueLength::<2>::decode_xdr(&mut &wire[..]),
            Err(XdrError::LengthLimit {
                length: 3,
                maximum: 2
            })
        );
        assert!(
            SegmentEncoder::default()
                .opaque(&Bytes::from_static(b"abc"), Some(2))
                .is_err()
        );
    }

    #[test]
    fn multiple_owners_form_one_body_with_combined_padding() {
        let first = Bytes::from_owner(vec![1, 2, 3]);
        let second = Bytes::from_owner(vec![4, 5]);
        let value = OpaqueSegments::<5>::try_new(vec![first.clone(), second.clone()]).unwrap();
        let segments = value.to_xdr_segments().unwrap();
        assert_eq!(segments[1].as_ptr(), first.as_ptr());
        assert_eq!(segments[2].as_ptr(), second.as_ptr());
        let actual: Vec<u8> = segments
            .iter()
            .flat_map(|segment| segment.iter().copied())
            .collect();
        assert_eq!(actual, [0, 0, 0, 5, 1, 2, 3, 4, 5, 0, 0, 0]);
        assert_eq!(actual, value.to_xdr_bytes().unwrap());
        assert_eq!(
            OpaqueSegments::<5>::from_xdr_bytes(&actual).unwrap(),
            OpaqueSegments::from(Bytes::from_static(&[1, 2, 3, 4, 5]))
        );
        assert!(OpaqueSegments::<4>::try_new(vec![first, second]).is_err());
        assert!(OpaqueSegments::<4>::from_xdr_bytes(&actual).is_err());
    }
}
