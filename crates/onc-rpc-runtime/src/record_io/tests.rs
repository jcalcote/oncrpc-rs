use super::*;
use std::{
    collections::VecDeque,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::ReadBuf;

struct ShortReader {
    input: Vec<u8>,
    position: usize,
    max_read: usize,
    // A payload range on the wire and its expected caller destination.
    direct: Option<(std::ops::Range<usize>, usize)>,
    failed: bool,
}
impl ShortReader {
    fn new(input: Vec<u8>, max_read: usize) -> Self {
        Self {
            input,
            position: 0,
            max_read,
            direct: None,
            failed: false,
        }
    }
}
impl AsyncRead for ShortReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.failed {
            return Poll::Ready(Err(io::ErrorKind::ConnectionReset.into()));
        }
        if let Some((range, destination)) = &self.direct {
            if range.contains(&self.position) {
                assert_eq!(
                    output.initialize_unfilled().as_ptr() as usize,
                    destination + self.position - range.start
                );
            }
        }
        let count = self
            .max_read
            .min(output.remaining())
            .min(self.input.len() - self.position);
        output.put_slice(&self.input[self.position..self.position + count]);
        self.position += count;
        Poll::Ready(Ok(()))
    }
}

fn fragment(payload: &[u8], last: bool) -> Vec<u8> {
    let mut output = ((if last { 0x8000_0000 } else { 0 }) | payload.len() as u32)
        .to_be_bytes()
        .to_vec();
    output.extend_from_slice(payload);
    output
}

#[tokio::test]
async fn short_reads_fill_caller_storage_directly() {
    let mut output = [0; 10];
    let mut input = ShortReader::new(fragment(b"abcdefghij", true), 2);
    input.direct = Some((4..14, output.as_mut_ptr() as usize));
    let mut record = RecordReader::new(&mut input, RecordLimits::default()).unwrap();
    record.read_exact(&mut output).await.unwrap();
    assert_eq!(&output, b"abcdefghij");
    assert_eq!(record.read_some(&mut [0]).await.unwrap(), 0);
}

#[tokio::test]
async fn fragments_cross_caller_segment_boundaries_without_consuming_next_record() {
    let mut wire = fragment(b"ab", false);
    wire.extend(fragment(&[], false));
    wire.extend(fragment(b"cdef", true));
    wire.extend(fragment(b"next", true));
    let mut input = ShortReader::new(wire, 1);
    {
        let mut record = RecordReader::new(&mut input, RecordLimits::default()).unwrap();
        let mut first = [0; 3];
        let mut second = [0; 3];
        record.read_exact(&mut first).await.unwrap();
        record.read_exact(&mut second).await.unwrap();
        assert_eq!(&first, b"abc");
        assert_eq!(&second, b"def");
        assert_eq!(record.read_some(&mut [0]).await.unwrap(), 0);
        assert_eq!(record.read_some(&mut [0]).await.unwrap(), 0);
    }
    assert_eq!(input.position, 18);
    let mut next = RecordReader::new(&mut input, RecordLimits::default()).unwrap();
    let mut output = [0; 4];
    next.read_exact(&mut output).await.unwrap();
    assert_eq!(&output, b"next");
}

#[tokio::test]
async fn empty_records_and_exact_reads_have_unambiguous_boundaries() {
    let mut wire = fragment(&[], false);
    wire.extend(fragment(&[], true));
    let mut input = ShortReader::new(wire, 1);
    let mut record = RecordReader::new(&mut input, RecordLimits::default()).unwrap();
    record.read_exact(&mut []).await.unwrap();
    assert_eq!(record.read_some(&mut [0]).await.unwrap(), 0);
    assert!(
        record
            .read_some(&mut [])
            .await
            .unwrap_err()
            .to_string()
            .contains("empty")
    );
    assert_eq!(
        record.read_exact(&mut [0]).await.unwrap_err(),
        RuntimeError::Wire(crate::WireError::UnexpectedEof {
            needed: 1,
            remaining: 0
        })
    );
}

#[tokio::test]
async fn record_limits_include_every_marker_and_are_checked_before_payload_read() {
    let mut wire = fragment(b"ab", false);
    wire.extend(fragment(b"cd", true));
    let mut input = ShortReader::new(wire, 4);
    let mut record = RecordReader::new(
        &mut input,
        RecordLimits {
            max_record_bytes: 11,
            max_fragments: 2,
        },
    )
    .unwrap();
    assert!(
        record
            .read_exact(&mut [0; 4])
            .await
            .unwrap_err()
            .to_string()
            .contains("configured limit")
    );
    assert_eq!(input.position, 10, "second payload must not be read");
}

#[tokio::test]
async fn exact_record_and_fragment_limits_are_permitted() {
    let mut wire = fragment(b"ab", false);
    wire.extend(fragment(b"cd", true));
    let mut input = ShortReader::new(wire, 3);
    let mut record = RecordReader::new(
        &mut input,
        RecordLimits {
            max_record_bytes: 12,
            max_fragments: 2,
        },
    )
    .unwrap();
    let mut output = [0; 4];
    record.read_exact(&mut output).await.unwrap();
    assert_eq!(&output, b"abcd");
    assert_eq!(record.read_some(&mut [0]).await.unwrap(), 0);
}

#[tokio::test]
async fn excessive_empty_fragments_fail_without_reading_another_marker() {
    let mut wire = fragment(&[], false);
    wire.extend(fragment(&[], false));
    wire.extend(fragment(b"x", true));
    let mut input = ShortReader::new(wire, 2);
    let mut record = RecordReader::new(
        &mut input,
        RecordLimits {
            max_record_bytes: 99,
            max_fragments: 2,
        },
    )
    .unwrap();
    assert!(
        record
            .read_some(&mut [0])
            .await
            .unwrap_err()
            .to_string()
            .contains("too many fragments")
    );
    assert_eq!(input.position, 8);
}

#[tokio::test]
async fn malformed_lengths_early_eof_and_io_failures_are_distinct() {
    for wire in [vec![0x80, 0], vec![0x80, 0, 0, 2, b'a']] {
        let mut input = ShortReader::new(wire, 1);
        let mut record = RecordReader::new(&mut input, RecordLimits::default()).unwrap();
        assert_eq!(
            record.read_exact(&mut [0; 2]).await.unwrap_err(),
            RuntimeError::ConnectionClosed
        );
    }
    let mut input = ShortReader::new(fragment(b"x", true), 1);
    input.failed = true;
    let mut record = RecordReader::new(&mut input, RecordLimits::default()).unwrap();
    assert!(matches!(
        record.read_some(&mut [0]).await,
        Err(RuntimeError::Transport(_))
    ));
    let mut input = ShortReader::new(vec![0xff; 4], 4);
    let mut record = RecordReader::new(&mut input, RecordLimits::default()).unwrap();
    assert!(
        record
            .read_some(&mut [0])
            .await
            .unwrap_err()
            .to_string()
            .contains("configured limit")
    );
    assert_eq!(input.position, 4);
}

#[test]
fn invalid_limits_are_rejected_before_any_io() {
    let mut input = ShortReader::new(vec![], 1);
    for limits in [
        RecordLimits {
            max_record_bytes: 3,
            max_fragments: 1,
        },
        RecordLimits {
            max_record_bytes: 4,
            max_fragments: 0,
        },
    ] {
        assert!(RecordReader::new(&mut input, limits).is_err());
    }
    assert_eq!(input.position, 0);
}

struct ShortWriter {
    output: Vec<u8>,
    max_write: usize,
    segments: Vec<Bytes>,
    errors: VecDeque<io::ErrorKind>,
    vectored_calls: usize,
}
impl ShortWriter {
    fn new(segments: Vec<Bytes>, max_write: usize) -> Self {
        Self {
            output: Vec::new(),
            max_write,
            segments,
            errors: VecDeque::new(),
            vectored_calls: 0,
        }
    }
    fn expected_pointer(&self, mut offset: usize) -> usize {
        for segment in &self.segments {
            if offset < segment.len() {
                return segment.as_ptr() as usize + offset;
            }
            offset -= segment.len();
        }
        panic!("payload offset outside segments");
    }
}
impl AsyncWrite for ShortWriter {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, _: &[u8]) -> Poll<io::Result<usize>> {
        panic!("writer must use vectored operation");
    }
    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        slices: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        self.vectored_calls += 1;
        assert!(slices.len() <= 64);
        if let Some(kind) = self.errors.pop_front() {
            return Poll::Ready(Err(kind.into()));
        }
        let mut position = self.output.len();
        for slice in slices {
            assert!(!slice.is_empty());
            if position >= 4 {
                assert_eq!(
                    slice.as_ptr() as usize,
                    self.expected_pointer(position - 4),
                    "original segment pointer must reach the writer"
                );
            }
            position += slice.len();
        }
        let mut count = 0;
        for slice in slices {
            let accepted = slice.len().min(self.max_write - count);
            self.output.extend_from_slice(&slice[..accepted]);
            count += accepted;
            if count == self.max_write {
                break;
            }
        }
        Poll::Ready(Ok(count))
    }
    fn is_write_vectored(&self) -> bool {
        true
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn vectored_short_writes_keep_original_segment_pointers() {
    let segments = vec![
        Bytes::new(),
        Bytes::from_static(b"abc"),
        Bytes::new(),
        Bytes::from_static(b"defgh"),
        Bytes::new(),
    ];
    for max_write in 1..=12 {
        let mut writer = ShortWriter::new(segments.clone(), max_write);
        write_segments(&mut writer, &segments).await.unwrap();
        assert_eq!(writer.output, fragment(b"abcdefgh", true));
    }
}

#[tokio::test]
async fn vectored_batch_limit_does_not_limit_segment_count() {
    let segments = (0..200)
        .map(|i| Bytes::from(vec![i as u8; 3]))
        .collect::<Vec<_>>();
    let mut writer = ShortWriter::new(segments.clone(), usize::MAX);
    write_segments(&mut writer, &segments).await.unwrap();
    assert_eq!(writer.vectored_calls, 4);
    let payload = segments
        .iter()
        .flat_map(|segment| segment.iter().copied())
        .collect::<Vec<_>>();
    assert_eq!(writer.output, fragment(&payload, true));
}

#[tokio::test]
async fn empty_segment_lists_emit_an_empty_last_fragment() {
    for segments in [vec![], vec![Bytes::new(), Bytes::new()]] {
        let mut writer = ShortWriter::new(segments.clone(), 1);
        write_segments(&mut writer, &segments).await.unwrap();
        assert_eq!(writer.output, fragment(&[], true));
    }
}

#[tokio::test]
async fn writer_retries_interrupted_but_reports_zero_and_other_errors() {
    let segments = vec![Bytes::from_static(b"payload")];
    let mut writer = ShortWriter::new(segments.clone(), 3);
    writer.errors.push_back(io::ErrorKind::Interrupted);
    write_segments(&mut writer, &segments).await.unwrap();
    assert_eq!(writer.output, fragment(b"payload", true));
    let mut writer = ShortWriter::new(segments.clone(), 0);
    assert!(
        write_segments(&mut writer, &segments)
            .await
            .unwrap_err()
            .to_string()
            .contains("returned zero")
    );
    let mut writer = ShortWriter::new(segments.clone(), 3);
    writer.errors.push_back(io::ErrorKind::BrokenPipe);
    assert!(matches!(
        write_segments(&mut writer, &segments).await,
        Err(RuntimeError::Transport(_))
    ));
    assert!(writer.output.is_empty());
}

#[tokio::test]
async fn oversized_segment_total_fails_before_writing_or_coalescing() {
    // Repeated references share one small allocation; no 2 GiB test allocation.
    let chunk = Bytes::from(vec![0; 1024 * 1024]);
    let segments = vec![chunk; 2048];
    let mut writer = ShortWriter::new(vec![], 1);
    assert!(
        write_segments(&mut writer, &segments)
            .await
            .unwrap_err()
            .to_string()
            .contains("single-fragment limit")
    );
    assert_eq!(writer.vectored_calls, 0);
    assert!(writer.output.is_empty());
}

#[tokio::test]
async fn streams_without_vectored_support_use_the_scalar_fallback_without_coalescing() {
    struct ScalarWriter(Vec<u8>);
    impl AsyncWrite for ScalarWriter {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
            bytes: &[u8],
        ) -> Poll<io::Result<usize>> {
            let count = bytes.len().min(2);
            self.0.extend_from_slice(&bytes[..count]);
            Poll::Ready(Ok(count))
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }
    let mut writer = ScalarWriter(Vec::new());
    assert!(!writer.is_write_vectored());
    write_segments(
        &mut writer,
        &[Bytes::from_static(b"first"), Bytes::from_static(b"second")],
    )
    .await
    .unwrap();
    assert_eq!(writer.0, fragment(b"firstsecond", true));
}

#[tokio::test]
async fn cancelled_reads_preserve_each_partial_record_marker() {
    use std::{future::Future, task::Poll};
    use tokio::io::{AsyncWriteExt, duplex};
    for prefix in 0..4 {
        let (mut input, mut peer) = duplex(128);
        let wire = fragment(b"data", true);
        peer.write_all(&wire[..prefix]).await.unwrap();
        let mut record = RecordReader::new(&mut input, RecordLimits::default()).unwrap();
        let mut output = [0; 4];
        {
            let mut read = Box::pin(record.read_some(&mut output));
            std::future::poll_fn(|cx| {
                assert!(read.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
        }
        peer.write_all(&wire[prefix..]).await.unwrap();
        record.read_exact(&mut output).await.unwrap();
        assert_eq!(&output, b"data", "marker prefix {prefix}");
        assert_eq!(record.read_some(&mut [0]).await.unwrap(), 0);
    }
}
