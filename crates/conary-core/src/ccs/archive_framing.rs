// conary-core/src/ccs/archive_framing.rs

//! Canonical fixed-block MGZIP framing and ordered parallel decode.

use anyhow::{Context, Result, ensure};
use flate2::{Decompress, FlushDecompress, Status};
use std::io::{self, Cursor, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;

const HEADER_BYTES: usize = 20;
const FOOTER_BYTES: usize = 8;
const FIXED_HEADER_PREFIX: [u8; 16] =
    [0x1f, 0x8b, 8, 4, 0, 0, 0, 0, 0, 255, 8, 0, b'I', b'G', 4, 0];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ArchiveDecodeMetrics {
    pub(crate) workers: u64,
    pub(crate) blocks: u64,
    pub(crate) decoded_bytes: u64,
    pub(crate) block_bytes: u64,
    pub(crate) buffer_ceiling_bytes: u64,
}

struct MgzipFrame {
    deflate: Vec<u8>,
    crc32: u32,
    decoded_bytes: usize,
}

struct CanonicalFrameReader<R> {
    inner: R,
    blocks: u64,
    decoded_bytes: u64,
    previous_decoded_bytes: Option<usize>,
    finished: bool,
}

impl<R: Read> CanonicalFrameReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            blocks: 0,
            decoded_bytes: 0,
            previous_decoded_bytes: None,
            finished: false,
        }
    }

    fn next_frame(&mut self) -> io::Result<Option<MgzipFrame>> {
        if self.finished {
            return Ok(None);
        }

        let mut header = [0_u8; HEADER_BYTES];
        match self.inner.read(&mut header[..1])? {
            0 => {
                self.finished = true;
                if self.blocks == 0 {
                    return Err(invalid_data(
                        "CCS archive contains no canonical MGZIP blocks",
                    ));
                }
                return Ok(None);
            }
            1 => {}
            _ => unreachable!("one-byte read returned more than one byte"),
        }
        self.inner
            .read_exact(&mut header[1..])
            .map_err(|error| invalid_data(format!("truncated canonical MGZIP header: {error}")))?;
        if header[..16] != FIXED_HEADER_PREFIX {
            return Err(invalid_data("CCS archive has a noncanonical MGZIP header"));
        }
        if self
            .previous_decoded_bytes
            .is_some_and(|bytes| bytes != crate::ccs::CCS_BUDGET.archive_compression_block_bytes)
        {
            return Err(invalid_data(
                "CCS archive has a short MGZIP block before its final block",
            ));
        }

        let frame_bytes = u32::from_le_bytes(header[16..20].try_into().unwrap()) as usize;
        let encoded_ceiling = usize::try_from(
            crate::ccs::CCS_BUDGET
                .archive_encoded_block_ceiling_bytes()
                .map_err(invalid_budget)?,
        )
        .map_err(|_| invalid_data("canonical MGZIP frame ceiling exceeds usize"))?;
        if !(HEADER_BYTES + FOOTER_BYTES..=encoded_ceiling).contains(&frame_bytes) {
            return Err(invalid_data(format!(
                "canonical MGZIP frame size {frame_bytes} exceeds admitted range"
            )));
        }

        let mut remainder = vec![0_u8; frame_bytes - HEADER_BYTES];
        self.inner
            .read_exact(&mut remainder)
            .map_err(|error| invalid_data(format!("truncated canonical MGZIP frame: {error}")))?;
        let footer = remainder
            .len()
            .checked_sub(FOOTER_BYTES)
            .context("canonical MGZIP frame is shorter than its footer")
            .map_err(invalid_budget)?;
        let crc32 = u32::from_le_bytes(remainder[footer..footer + 4].try_into().unwrap());
        let decoded_bytes =
            u32::from_le_bytes(remainder[footer + 4..].try_into().unwrap()) as usize;
        if decoded_bytes == 0
            || decoded_bytes > crate::ccs::CCS_BUDGET.archive_compression_block_bytes
        {
            return Err(invalid_data(format!(
                "canonical MGZIP block declares invalid decoded size {decoded_bytes}"
            )));
        }
        self.blocks = self
            .blocks
            .checked_add(1)
            .ok_or_else(|| invalid_data("canonical MGZIP block count overflow"))?;
        self.decoded_bytes = self
            .decoded_bytes
            .checked_add(decoded_bytes as u64)
            .ok_or_else(|| invalid_data("canonical MGZIP decoded-byte count overflow"))?;
        self.previous_decoded_bytes = Some(decoded_bytes);
        remainder.truncate(footer);
        Ok(Some(MgzipFrame {
            deflate: remainder,
            crc32,
            decoded_bytes,
        }))
    }

    fn into_parts(self) -> (R, u64, u64) {
        (self.inner, self.blocks, self.decoded_bytes)
    }
}

fn decode_frame(frame: MgzipFrame) -> io::Result<Vec<u8>> {
    let mut decoder = Decompress::new(false);
    let mut output = Vec::with_capacity(frame.decoded_bytes);
    let status = decoder
        .decompress_vec(&frame.deflate, &mut output, FlushDecompress::Finish)
        .map_err(|error| invalid_data(format!("invalid canonical MGZIP DEFLATE block: {error}")))?;
    if status != Status::StreamEnd
        || decoder.total_in() != frame.deflate.len() as u64
        || decoder.total_out() != frame.decoded_bytes as u64
        || output.len() != frame.decoded_bytes
    {
        return Err(invalid_data(
            "canonical MGZIP DEFLATE block did not finish at its exact declared bounds",
        ));
    }
    let mut crc = flate2::Crc::new();
    crc.update(&output);
    if crc.sum() != frame.crc32 {
        return Err(invalid_data(format!(
            "canonical MGZIP block CRC mismatch: expected {:08x}, got {:08x}",
            frame.crc32,
            crc.sum()
        )));
    }
    Ok(output)
}

/// Serial canonical decoder for diagnostic and format-identification callers.
pub(crate) struct MgzipDecoder<R: Read> {
    frames: CanonicalFrameReader<R>,
    current: Cursor<Vec<u8>>,
}

impl<R: Read> MgzipDecoder<R> {
    pub(crate) fn new(reader: R) -> Self {
        Self {
            frames: CanonicalFrameReader::new(reader),
            current: Cursor::new(Vec::new()),
        }
    }
}

impl<R: Read> Read for MgzipDecoder<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        loop {
            let read = self.current.read(output)?;
            if read != 0 {
                return Ok(read);
            }
            let Some(frame) = self.frames.next_frame()? else {
                return Ok(0);
            };
            self.current = Cursor::new(decode_frame(frame)?);
        }
    }
}

struct DecodeJob {
    frame: MgzipFrame,
    result: mpsc::SyncSender<io::Result<Vec<u8>>>,
}

/// Ordered reader backed by one bounded worker pool.
pub(crate) struct ParallelMgzipDecoder<R: Read + Send + 'static> {
    ordered: Option<mpsc::Receiver<mpsc::Receiver<io::Result<Vec<u8>>>>>,
    current: Cursor<Vec<u8>>,
    coordinator: Option<JoinHandle<io::Result<CanonicalFrameReader<R>>>>,
    failed: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    workers: usize,
}

impl<R: Read + Send + 'static> ParallelMgzipDecoder<R> {
    pub(crate) fn new(reader: R, workers: usize) -> Result<Self> {
        crate::ccs::CCS_BUDGET.admit_archive_cpu_workers(workers)?;
        ensure!(workers > 0, "archive decode requires at least one worker");
        let (ordered_sender, ordered) = mpsc::sync_channel(workers.saturating_mul(2));
        let failed = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::new(AtomicBool::new(false));
        let coordinator_failed = Arc::clone(&failed);
        let coordinator_cancelled = Arc::clone(&cancelled);
        let coordinator = std::thread::spawn(move || {
            let mut job_senders = Vec::with_capacity(workers);
            let mut worker_handles = Vec::with_capacity(workers);
            for _ in 0..workers {
                let (job_sender, job_receiver) = mpsc::sync_channel::<DecodeJob>(1);
                let worker_failed = Arc::clone(&coordinator_failed);
                let worker_cancelled = Arc::clone(&coordinator_cancelled);
                worker_handles.push(std::thread::spawn(move || {
                    while !worker_cancelled.load(Ordering::Acquire) {
                        let job =
                            match job_receiver.recv_timeout(std::time::Duration::from_millis(10)) {
                                Ok(job) => job,
                                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                            };
                        let result = decode_frame(job.frame);
                        if result.is_err() {
                            worker_failed.store(true, Ordering::Release);
                            worker_cancelled.store(true, Ordering::Release);
                        }
                        if job.result.send(result).is_err() {
                            worker_cancelled.store(true, Ordering::Release);
                            break;
                        }
                    }
                }));
                job_senders.push(job_sender);
            }

            let mut frames = CanonicalFrameReader::new(reader);
            let mut next_worker = 0_usize;
            let dispatch = (|| -> io::Result<()> {
                while let Some(frame) = frames.next_frame()? {
                    if coordinator_cancelled.load(Ordering::Acquire) {
                        return Err(io::Error::new(
                            io::ErrorKind::Interrupted,
                            "archive decode was cancelled",
                        ));
                    }
                    let (result_sender, result_receiver) = mpsc::sync_channel(1);
                    send_cancellable(
                        &ordered_sender,
                        result_receiver,
                        &coordinator_cancelled,
                        "archive decode consumer closed",
                    )?;
                    send_cancellable(
                        &job_senders[next_worker],
                        DecodeJob {
                            frame,
                            result: result_sender,
                        },
                        &coordinator_cancelled,
                        "archive decoder worker closed",
                    )?;
                    next_worker = (next_worker + 1) % workers;
                }
                Ok(())
            })();
            drop(job_senders);
            for handle in worker_handles {
                if handle.join().is_err() {
                    coordinator_failed.store(true, Ordering::Release);
                    return Err(io::Error::other("archive decoder worker panicked"));
                }
            }
            dispatch?;
            if coordinator_failed.load(Ordering::Acquire) {
                return Err(invalid_data("canonical MGZIP worker rejected a block"));
            }
            Ok(frames)
        });
        Ok(Self {
            ordered: Some(ordered),
            current: Cursor::new(Vec::new()),
            coordinator: Some(coordinator),
            failed,
            cancelled,
            workers,
        })
    }

    pub(crate) fn finish(mut self) -> Result<(R, ArchiveDecodeMetrics)> {
        io::copy(&mut self, &mut io::sink())
            .context("finish ordered canonical MGZIP decode before joining workers")?;
        let coordinator = self
            .coordinator
            .take()
            .context("archive decode coordinator was already joined")?;
        let frames = coordinator
            .join()
            .map_err(|_| anyhow::anyhow!("archive decode coordinator panicked"))??;
        ensure!(
            !self.failed.load(Ordering::Acquire),
            "canonical MGZIP decode failed"
        );
        let (reader, blocks, decoded_bytes) = frames.into_parts();
        Ok((
            reader,
            ArchiveDecodeMetrics {
                workers: self.workers as u64,
                blocks,
                decoded_bytes,
                block_bytes: crate::ccs::CCS_BUDGET.archive_compression_block_bytes as u64,
                buffer_ceiling_bytes: crate::ccs::CCS_BUDGET
                    .archive_decode_buffer_ceiling_bytes(self.workers)?,
            },
        ))
    }
}

impl<R: Read + Send + 'static> Read for ParallelMgzipDecoder<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        loop {
            let read = self.current.read(output)?;
            if read != 0 {
                return Ok(read);
            }
            let result_receiver = match self
                .ordered
                .as_ref()
                .expect("archive decode receiver exists until finish")
                .recv()
            {
                Ok(receiver) => receiver,
                Err(_) => return Ok(0),
            };
            self.current = Cursor::new(result_receiver.recv().map_err(|_| {
                io::Error::new(io::ErrorKind::BrokenPipe, "archive decoder worker closed")
            })??);
        }
    }
}

impl<R: Read + Send + 'static> Drop for ParallelMgzipDecoder<R> {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        drop(self.ordered.take());
        if let Some(coordinator) = self.coordinator.take() {
            let _ = coordinator.join();
        }
    }
}

fn send_cancellable<T>(
    sender: &mpsc::SyncSender<T>,
    mut value: T,
    cancelled: &AtomicBool,
    disconnected: &'static str,
) -> io::Result<()> {
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "archive decode was cancelled",
            ));
        }
        match sender.try_send(value) {
            Ok(()) => return Ok(()),
            Err(mpsc::TrySendError::Full(returned)) => {
                value = returned;
                std::thread::sleep(std::time::Duration::from_micros(100));
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, disconnected));
            }
        }
    }
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn invalid_budget(error: impl std::fmt::Display) -> io::Error {
    invalid_data(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gzp::ZWriter;
    use gzp::deflate::Mgzip;
    use gzp::par::compress::ParCompressBuilder;
    use std::io::Write;

    fn payload() -> Vec<u8> {
        (0..(crate::ccs::CCS_BUDGET.archive_compression_block_bytes * 3 + 17))
            .map(|index| ((index * 31 + index / 251) % 256) as u8)
            .collect()
    }

    fn encode(bytes: &[u8], block_bytes: usize) -> Vec<u8> {
        let mut encoder = ParCompressBuilder::<Mgzip>::new()
            .buffer_size(block_bytes)
            .unwrap()
            .num_threads(3)
            .unwrap()
            .compression_level(flate2::Compression::default())
            .from_writer(Vec::new());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    fn decode(bytes: Vec<u8>, workers: usize) -> Result<(Vec<u8>, ArchiveDecodeMetrics)> {
        let mut decoder = ParallelMgzipDecoder::new(Cursor::new(bytes), workers)?;
        let mut decoded = Vec::new();
        decoder.read_to_end(&mut decoded)?;
        let (_, metrics) = decoder.finish()?;
        Ok((decoded, metrics))
    }

    fn frame_ranges(bytes: &[u8]) -> Vec<std::ops::Range<usize>> {
        let mut ranges = Vec::new();
        let mut offset = 0_usize;
        while offset < bytes.len() {
            let frame_bytes =
                u32::from_le_bytes(bytes[offset + 16..offset + 20].try_into().unwrap()) as usize;
            ranges.push(offset..offset + frame_bytes);
            offset += frame_bytes;
        }
        assert_eq!(offset, bytes.len());
        ranges
    }

    #[test]
    fn ordered_parallel_decode_round_trips_exact_bounded_geometry() {
        let payload = payload();
        let encoded = encode(
            &payload,
            crate::ccs::CCS_BUDGET.archive_compression_block_bytes,
        );
        let (decoded, metrics) = decode(encoded, 4).unwrap();

        assert_eq!(decoded, payload);
        assert_eq!(metrics.workers, 4);
        assert_eq!(metrics.blocks, 4);
        assert_eq!(metrics.decoded_bytes, payload.len() as u64);
        assert_eq!(
            metrics.block_bytes,
            crate::ccs::CCS_BUDGET.archive_compression_block_bytes as u64
        );
        assert_eq!(
            metrics.buffer_ceiling_bytes,
            crate::ccs::CCS_BUDGET
                .archive_decode_buffer_ceiling_bytes(4)
                .unwrap()
        );
    }

    #[test]
    fn finish_drains_bounded_work_before_joining_workers() {
        let encoded = encode(
            &payload(),
            crate::ccs::CCS_BUDGET.archive_compression_block_bytes,
        );
        let decoder = ParallelMgzipDecoder::new(Cursor::new(encoded), 4).unwrap();

        let (_, metrics) = decoder.finish().unwrap();

        assert_eq!(metrics.workers, 4);
        assert_eq!(metrics.blocks, 4);
    }

    #[test]
    fn malformed_truncated_corrupt_and_trailing_frames_fail_closed() {
        let payload = payload();
        let canonical = encode(
            &payload,
            crate::ccs::CCS_BUDGET.archive_compression_block_bytes,
        );

        let mut malformed_header = canonical.clone();
        malformed_header[12] = b'X';
        assert!(decode(malformed_header, 3).is_err());

        let mut oversized = canonical.clone();
        oversized[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode(oversized, 3).is_err());

        let first_frame = frame_ranges(&canonical)[0].clone();
        let mut corrupt_crc = canonical.clone();
        corrupt_crc[first_frame.end - FOOTER_BYTES] ^= 0x80;
        assert!(decode(corrupt_crc, 3).is_err());

        let mut truncated = canonical.clone();
        truncated.pop();
        assert!(decode(truncated, 3).is_err());

        let mut trailing = canonical;
        trailing.push(0x1f);
        assert!(decode(trailing, 3).is_err());
    }

    #[test]
    fn reordered_and_substituted_frames_change_ordered_bytes_and_short_blocks_fail() {
        let payload = payload();
        let canonical = encode(
            &payload,
            crate::ccs::CCS_BUDGET.archive_compression_block_bytes,
        );
        let ranges = frame_ranges(&canonical);
        assert!(ranges.len() >= 4);

        let mut reordered = Vec::with_capacity(canonical.len());
        reordered.extend_from_slice(&canonical[ranges[1].clone()]);
        reordered.extend_from_slice(&canonical[ranges[0].clone()]);
        for range in &ranges[2..] {
            reordered.extend_from_slice(&canonical[range.clone()]);
        }
        let (decoded, _) = decode(reordered, 3).unwrap();
        assert_ne!(decoded, payload);

        let mut substituted = Vec::with_capacity(canonical.len());
        substituted.extend_from_slice(&canonical[ranges[0].clone()]);
        substituted.extend_from_slice(&canonical[ranges[0].clone()]);
        for range in &ranges[2..] {
            substituted.extend_from_slice(&canonical[range.clone()]);
        }
        let (decoded, _) = decode(substituted, 3).unwrap();
        assert_ne!(decoded, payload);

        let short_blocks = encode(
            &payload,
            crate::ccs::CCS_BUDGET.archive_compression_block_bytes / 2,
        );
        assert!(decode(short_blocks, 3).is_err());
    }

    #[test]
    fn ordinary_gzip_is_not_a_second_current_representation() {
        let payload = payload();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&payload).unwrap();
        let ordinary_gzip = encoder.finish().unwrap();

        assert!(decode(ordinary_gzip, 2).is_err());
    }
}
