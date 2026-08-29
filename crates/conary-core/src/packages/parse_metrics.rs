// conary-core/src/packages/parse_metrics.rs

//! Exact diagnostic work counters for native package archive parsing.

use std::io::{Read, Seek, SeekFrom};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Exact I/O and traversal work performed while parsing one native package.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NativePackageParseMetrics {
    pub source_archive_opens: u64,
    pub source_archive_bytes_read: u64,
    pub archive_passes: u64,
    pub archive_entries_traversed: u64,
    pub decompressed_archive_bytes_read: u64,
    pub intermediate_archive_bytes_written: u64,
    pub intermediate_archive_bytes_read: u64,
    pub intermediate_archive_file_syncs: u64,
    pub payload_files_spooled: u64,
    pub payload_bytes_spooled: u64,
    pub payload_spool_bytes_reread: u64,
    pub payload_spool_file_syncs: u64,
    pub payload_bytes_hashed: u64,
}

impl NativePackageParseMetrics {
    pub(crate) fn checked_add(&mut self, other: Self) -> crate::Result<()> {
        self.source_archive_opens = add(
            self.source_archive_opens,
            other.source_archive_opens,
            "native source archive open count",
        )?;
        self.source_archive_bytes_read = add(
            self.source_archive_bytes_read,
            other.source_archive_bytes_read,
            "native source archive read bytes",
        )?;
        self.archive_passes = add(
            self.archive_passes,
            other.archive_passes,
            "native archive pass count",
        )?;
        self.archive_entries_traversed = add(
            self.archive_entries_traversed,
            other.archive_entries_traversed,
            "native archive entry traversal count",
        )?;
        self.decompressed_archive_bytes_read = add(
            self.decompressed_archive_bytes_read,
            other.decompressed_archive_bytes_read,
            "native decompressed archive read bytes",
        )?;
        self.intermediate_archive_bytes_written = add(
            self.intermediate_archive_bytes_written,
            other.intermediate_archive_bytes_written,
            "native intermediate archive write bytes",
        )?;
        self.intermediate_archive_bytes_read = add(
            self.intermediate_archive_bytes_read,
            other.intermediate_archive_bytes_read,
            "native intermediate archive read bytes",
        )?;
        self.intermediate_archive_file_syncs = add(
            self.intermediate_archive_file_syncs,
            other.intermediate_archive_file_syncs,
            "native intermediate archive file sync count",
        )?;
        self.payload_files_spooled = add(
            self.payload_files_spooled,
            other.payload_files_spooled,
            "native payload spool file count",
        )?;
        self.payload_bytes_spooled = add(
            self.payload_bytes_spooled,
            other.payload_bytes_spooled,
            "native payload spool bytes",
        )?;
        self.payload_spool_bytes_reread = add(
            self.payload_spool_bytes_reread,
            other.payload_spool_bytes_reread,
            "native payload spool reread bytes",
        )?;
        self.payload_spool_file_syncs = add(
            self.payload_spool_file_syncs,
            other.payload_spool_file_syncs,
            "native payload spool file sync count",
        )?;
        self.payload_bytes_hashed = add(
            self.payload_bytes_hashed,
            other.payload_bytes_hashed,
            "native payload hash bytes",
        )?;
        Ok(())
    }
}

fn add(left: u64, right: u64, label: &str) -> crate::Result<u64> {
    left.checked_add(right)
        .ok_or_else(|| crate::Error::ParseError(format!("{label} overflow")))
}

/// Share an exact byte counter across readers consumed behind trait objects.
#[derive(Debug, Clone, Default)]
pub(crate) struct ReadCounter(Arc<AtomicU64>);

impl ReadCounter {
    pub(crate) fn wrap<R>(&self, reader: R) -> CountingReader<R> {
        CountingReader {
            reader,
            bytes: Arc::clone(&self.0),
        }
    }

    pub(crate) fn bytes(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

pub(crate) struct CountingReader<R> {
    reader: R,
    bytes: Arc<AtomicU64>,
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.reader.read(buffer)?;
        self.bytes
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(read as u64)
            })
            .map_err(|_| std::io::Error::other("native package read-byte counter overflow"))?;
        Ok(read)
    }
}

impl<R: Seek> Seek for CountingReader<R> {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.reader.seek(position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read, Seek};

    #[test]
    fn counted_reader_tracks_reads_but_not_rewinds() {
        let counter = ReadCounter::default();
        let mut reader = counter.wrap(Cursor::new(b"payload"));
        let mut first = [0_u8; 3];
        reader.read_exact(&mut first).unwrap();
        reader.seek(SeekFrom::Start(0)).unwrap();
        let mut all = Vec::new();
        reader.read_to_end(&mut all).unwrap();

        assert_eq!(&first, b"pay");
        assert_eq!(all, b"payload");
        assert_eq!(counter.bytes(), 10);
    }
}
