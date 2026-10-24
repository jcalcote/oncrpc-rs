//! Streaming record framing without payload assembly or coalescing.
use crate::{MAX_FRAGMENT_LEN, RecordLimits, RuntimeError};
use bytes::Bytes;
use std::io::{self, IoSlice};
use std::pin::Pin;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};

/// One record's cursor. Dropping an unfinished cursor does not resynchronize the
/// stream: its owner must retire the connection rather than start another record.
pub(crate) struct RecordReader<'a, R: AsyncRead + Unpin> {
    reader: &'a mut R,
    limits: RecordLimits,
    fragments: usize,
    record_bytes: usize,
    remaining: usize,
    last: bool,
    marker: [u8; 4],
    marker_used: usize,
}

impl<'a, R: AsyncRead + Unpin> RecordReader<'a, R> {
    pub(crate) fn new(reader: &'a mut R, limits: RecordLimits) -> Result<Self, RuntimeError> {
        if limits.max_record_bytes < 4 || limits.max_fragments == 0 {
            return Err(RuntimeError::Transport("invalid RPC record limits".into()));
        }
        Ok(Self {
            reader,
            limits,
            fragments: 0,
            record_bytes: 0,
            remaining: 0,
            last: false,
            marker: [0; 4],
            marker_used: 0,
        })
    }

    /// Fill caller storage directly, spanning record fragments as needed.
    pub(crate) async fn read_exact(&mut self, mut output: &mut [u8]) -> Result<(), RuntimeError> {
        while !output.is_empty() {
            let count = self.read_some(output).await?;
            if count == 0 {
                return Err(RuntimeError::Wire(crate::WireError::UnexpectedEof {
                    needed: output.len(),
                    remaining: 0,
                }));
            }
            output = &mut output[count..];
        }
        Ok(())
    }

    /// Read into nonempty caller storage; zero means the end of this record.
    /// The next record is never consumed. Empty output is rejected so it cannot
    /// be mistaken for the record boundary.
    pub(crate) async fn read_some(&mut self, output: &mut [u8]) -> Result<usize, RuntimeError> {
        if output.is_empty() {
            return Err(RuntimeError::Transport("empty RPC read destination".into()));
        }
        while self.remaining == 0 {
            if self.last {
                return Ok(0);
            }
            if self.fragments == self.limits.max_fragments {
                return Err(RuntimeError::Transport(
                    "RPC record has too many fragments".into(),
                ));
            }
            // Persist partial markers across cancelled read futures. `read`
            // consumes no bytes when it returns Pending, unlike read_exact.
            while self.marker_used < self.marker.len() {
                let count = self
                    .reader
                    .read(&mut self.marker[self.marker_used..])
                    .await
                    .map_err(read_error)?;
                if count == 0 {
                    return Err(RuntimeError::ConnectionClosed);
                }
                self.marker_used += count;
            }
            let marker = u32::from_be_bytes(self.marker);
            self.marker_used = 0;
            let length = usize::try_from(marker & MAX_FRAGMENT_LEN)
                .map_err(|_| RuntimeError::Transport("RPC fragment length overflow".into()))?;
            self.record_bytes = self
                .record_bytes
                .checked_add(4)
                .and_then(|total| total.checked_add(length))
                .filter(|total| *total <= self.limits.max_record_bytes)
                .ok_or_else(|| {
                    RuntimeError::Transport("RPC record exceeds configured limit".into())
                })?;
            self.fragments += 1;
            self.remaining = length;
            self.last = marker & 0x8000_0000 != 0;
        }
        let length = output.len().min(self.remaining);
        let count = self
            .reader
            .read(&mut output[..length])
            .await
            .map_err(read_error)?;
        if count == 0 {
            return Err(RuntimeError::ConnectionClosed);
        }
        self.remaining -= count;
        Ok(count)
    }
}

/// Emit a single last-fragment record from immutable owned segments. The only
/// temporary storage is a marker and a fixed-size stack array of slice metadata.
/// Callers retain ownership through this future and retire partial-frame writes
/// on cancellation or error.
pub(crate) async fn write_segments<W: AsyncWrite + Unpin + ?Sized>(
    writer: &mut W,
    segments: &[Bytes],
) -> Result<(), RuntimeError> {
    let length = segments.iter().try_fold(0usize, |total, segment| {
        total
            .checked_add(segment.len())
            .filter(|total| *total <= MAX_FRAGMENT_LEN as usize)
            .ok_or_else(|| {
                RuntimeError::Transport("RPC record exceeds single-fragment limit".into())
            })
    })?;
    let marker = (0x8000_0000 | length as u32).to_be_bytes();
    let mut index = 0;
    let mut offset = 0;
    loop {
        if index > segments.len() {
            return Ok(());
        }
        // Slice descriptors are needed only while polling the writer. Keeping
        // them inside the poll closure avoids a 64-entry array in every suspended
        // call's future; segment owners remain in the surrounding call.
        let result = std::future::poll_fn(|cx| {
            let mut slices = [IoSlice::new(&[]); 64];
            let mut count = 0;
            for current in index..=segments.len() {
                let bytes = if current == 0 {
                    &marker[..]
                } else {
                    &segments[current - 1][..]
                };
                let bytes = &bytes[if current == index { offset } else { 0 }..];
                if !bytes.is_empty() {
                    slices[count] = IoSlice::new(bytes);
                    count += 1;
                    if count == slices.len() {
                        break;
                    }
                }
            }
            if count == 0 {
                return std::task::Poll::Ready(Ok(0));
            }
            Pin::new(&mut *writer).poll_write_vectored(cx, &slices[..count])
        })
        .await;
        let mut written = match result {
            Ok(0) => {
                return Err(RuntimeError::Transport(
                    "RPC frame write returned zero".into(),
                ));
            }
            Ok(written) => written,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(RuntimeError::Transport(error.to_string())),
        };
        while index <= segments.len() {
            let available = if index == 0 {
                marker.len()
            } else {
                segments[index - 1].len()
            } - offset;
            if written < available {
                offset += written;
                break;
            }
            written -= available;
            index += 1;
            offset = 0;
        }
        if index > segments.len() {
            return Ok(());
        }
    }
}

fn read_error(error: io::Error) -> RuntimeError {
    if error.kind() == io::ErrorKind::UnexpectedEof {
        RuntimeError::ConnectionClosed
    } else {
        RuntimeError::Transport(error.to_string())
    }
}

#[cfg(test)]
mod tests;
