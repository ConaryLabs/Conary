// conary-core/src/ccs/policy/content.rs
//! Disk-backed content transforms for CCS build policy.

use super::{BuildPolicy, PolicyAction, PolicyContext, PolicyError};
use anyhow::Result;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

const STREAM_BUFFER_SIZE: usize = crate::packages::payload::PAYLOAD_IO_BUFFER_SIZE;

#[derive(Default)]
pub struct StripBinariesPolicy;

impl StripBinariesPolicy {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl BuildPolicy for StripBinariesPolicy {
    fn name(&self) -> &str {
        "StripBinaries"
    }

    fn apply(&self, ctx: &mut PolicyContext<'_>) -> Result<PolicyAction> {
        if !ctx.entry.node.kind.is_regular()
            || (!is_elf(ctx.source_path)?
                || (ctx.entry.node.mode & 0o111 == 0 && !ctx.entry.path.contains(".so")))
        {
            return Ok(PolicyAction::Keep);
        }

        let original_size = fs::metadata(ctx.source_path)?.len();
        if strip_elf_with_system_tool(ctx.source_path, ctx.replacement_path).is_ok() {
            let stripped_size = fs::metadata(ctx.replacement_path)?.len();
            if stripped_size < original_size {
                log::debug!(
                    "Stripped {} with system strip: {} -> {} bytes",
                    ctx.entry.path,
                    original_size,
                    stripped_size
                );
                return Ok(PolicyAction::Replace(ctx.replacement_path.to_path_buf()));
            }
            return Ok(PolicyAction::Keep);
        }

        match strip_elf_streaming(ctx.source_path, ctx.replacement_path) {
            Ok(stripped_size) if stripped_size < original_size => {
                log::debug!(
                    "Stripped {}: {} -> {} bytes (native)",
                    ctx.entry.path,
                    original_size,
                    stripped_size
                );
                Ok(PolicyAction::Replace(ctx.replacement_path.to_path_buf()))
            }
            Ok(_) => Ok(PolicyAction::Keep),
            Err(error) => {
                log::debug!("strip failed for {}: {}", ctx.entry.path, error);
                Ok(PolicyAction::Keep)
            }
        }
    }
}

fn is_elf(path: &Path) -> io::Result<bool> {
    let mut file = File::open(path)?;
    let mut magic = [0_u8; 4];
    Ok(file.read(&mut magic)? == magic.len() && magic == *b"\x7fELF")
}

fn strip_elf_with_system_tool(source: &Path, output: &Path) -> std::result::Result<(), String> {
    fs::copy(source, output).map_err(|error| format!("Failed to copy for stripping: {error}"))?;
    for tool in ["strip", "llvm-strip"] {
        if std::process::Command::new(tool)
            .arg("--strip-unneeded")
            .arg(output)
            .output()
            .is_ok_and(|result| result.status.success())
        {
            return Ok(());
        }
    }
    Err("No system strip tool available".to_string())
}

fn strip_elf_streaming(source: &Path, output: &Path) -> std::result::Result<u64, String> {
    let mut input = File::open(source).map_err(|error| error.to_string())?;
    let source_size = input.metadata().map_err(|error| error.to_string())?.len();
    let mut header = [0_u8; 64];
    input
        .read_exact(&mut header)
        .map_err(|error| format!("Failed to read ELF header: {error}"))?;
    if header[..4] != *b"\x7fELF" {
        return Err("Not an ELF file".to_string());
    }
    let class = header[4];
    let little_endian = match header[5] {
        1 => true,
        2 => false,
        _ => return Err("Unsupported ELF byte order".to_string()),
    };
    let read_u16 = |bytes: &[u8]| {
        if little_endian {
            u16::from_le_bytes(bytes.try_into().expect("two-byte ELF field"))
        } else {
            u16::from_be_bytes(bytes.try_into().expect("two-byte ELF field"))
        }
    };
    let read_u32 = |bytes: &[u8]| {
        if little_endian {
            u32::from_le_bytes(bytes.try_into().expect("four-byte ELF field"))
        } else {
            u32::from_be_bytes(bytes.try_into().expect("four-byte ELF field"))
        }
    };
    let read_u64 = |bytes: &[u8]| {
        if little_endian {
            u64::from_le_bytes(bytes.try_into().expect("eight-byte ELF field"))
        } else {
            u64::from_be_bytes(bytes.try_into().expect("eight-byte ELF field"))
        }
    };

    let e_type = read_u16(&header[16..18]);
    if !matches!(e_type, 2 | 3) {
        return Err("Not an executable or shared library".to_string());
    }
    let (header_size, phoff, phentsize, phnum, expected_phentsize) = match class {
        1 => (
            u64::from(read_u16(&header[40..42])),
            u64::from(read_u32(&header[28..32])),
            u64::from(read_u16(&header[42..44])),
            u64::from(read_u16(&header[44..46])),
            32_usize,
        ),
        2 => (
            u64::from(read_u16(&header[52..54])),
            read_u64(&header[32..40]),
            u64::from(read_u16(&header[54..56])),
            u64::from(read_u16(&header[56..58])),
            56_usize,
        ),
        _ => return Err("Unsupported ELF class".to_string()),
    };
    if phentsize < expected_phentsize as u64 {
        return Err("ELF program-header entry is too short".to_string());
    }

    let mut last_segment_end = 0_u64;
    let mut program_header = [0_u8; 56];
    for index in 0..phnum {
        let offset = phoff
            .checked_add(
                index
                    .checked_mul(phentsize)
                    .ok_or("ELF program-header offset overflow")?,
            )
            .ok_or("ELF program-header offset overflow")?;
        input
            .seek(SeekFrom::Start(offset))
            .and_then(|_| input.read_exact(&mut program_header[..expected_phentsize]))
            .map_err(|error| format!("Failed to read ELF program header: {error}"))?;
        if read_u32(&program_header[..4]) != 1 {
            continue;
        }
        let (segment_offset, segment_size) = if class == 1 {
            (
                u64::from(read_u32(&program_header[4..8])),
                u64::from(read_u32(&program_header[16..20])),
            )
        } else {
            (
                read_u64(&program_header[8..16]),
                read_u64(&program_header[32..40]),
            )
        };
        last_segment_end = last_segment_end.max(
            segment_offset
                .checked_add(segment_size)
                .ok_or("ELF segment extent overflow")?,
        );
    }

    let phdr_end = phoff
        .checked_add(
            phnum
                .checked_mul(phentsize)
                .ok_or("ELF program-header table overflow")?,
        )
        .ok_or("ELF program-header table overflow")?;
    let truncate_at = last_segment_end.max(phdr_end).max(header_size);
    if truncate_at >= source_size {
        return Err("No sections to strip".to_string());
    }

    input
        .seek(SeekFrom::Start(0))
        .map_err(|error| error.to_string())?;
    let mut output_file = File::create(output).map_err(|error| error.to_string())?;
    let copied = io::copy(
        &mut Read::by_ref(&mut input).take(truncate_at),
        &mut output_file,
    )
    .map_err(|error| error.to_string())?;
    if copied != truncate_at {
        return Err(format!(
            "ELF source ended early while stripping: expected {truncate_at}, got {copied}"
        ));
    }
    match class {
        1 => {
            write_zeros(&mut output_file, 32, 4)?;
            write_zeros(&mut output_file, 48, 4)?;
        }
        2 => {
            write_zeros(&mut output_file, 40, 8)?;
            write_zeros(&mut output_file, 60, 4)?;
        }
        _ => unreachable!(),
    }
    Ok(truncate_at)
}

fn write_zeros(file: &mut File, offset: u64, count: usize) -> std::result::Result<(), String> {
    file.seek(SeekFrom::Start(offset))
        .and_then(|_| file.write_all(&[0_u8; 8][..count]))
        .map_err(|error| error.to_string())
}

pub struct FixShebangsPolicy {
    replacements: HashMap<String, String>,
}

impl FixShebangsPolicy {
    #[must_use]
    pub fn new(replacements: HashMap<String, String>) -> Self {
        Self { replacements }
    }
}

impl BuildPolicy for FixShebangsPolicy {
    fn name(&self) -> &str {
        "FixShebangs"
    }

    fn apply(&self, ctx: &mut PolicyContext<'_>) -> Result<PolicyAction> {
        if !ctx.entry.node.kind.is_regular() || !has_prefix(ctx.source_path, b"#!")? {
            return Ok(PolicyAction::Keep);
        }
        if !first_line_is_utf8(ctx.source_path)? {
            return Ok(PolicyAction::Keep);
        }
        for (old, new) in &self.replacements {
            if !old.is_empty() && first_line_contains(ctx.source_path, old.as_bytes())? {
                rewrite_first_line(
                    ctx.source_path,
                    ctx.replacement_path,
                    old.as_bytes(),
                    new.as_bytes(),
                )?;
                return Ok(PolicyAction::Replace(ctx.replacement_path.to_path_buf()));
            }
        }
        Ok(PolicyAction::Keep)
    }
}

fn has_prefix(path: &Path, expected: &[u8]) -> io::Result<bool> {
    let mut file = File::open(path)?;
    let mut prefix = vec![0_u8; expected.len()];
    Ok(file.read(&mut prefix)? == expected.len() && prefix == expected)
}

fn first_line_is_utf8(path: &Path) -> io::Result<bool> {
    let mut reader = File::open(path)?;
    let mut buffer = [0_u8; STREAM_BUFFER_SIZE];
    let mut carry = Vec::with_capacity(3);
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            return Ok(carry.is_empty());
        }
        let line_count = buffer[..count]
            .iter()
            .position(|byte| *byte == b'\n')
            .unwrap_or(count);
        let mut candidate = Vec::with_capacity(carry.len() + line_count);
        candidate.extend_from_slice(&carry);
        candidate.extend_from_slice(&buffer[..line_count]);
        match std::str::from_utf8(&candidate) {
            Ok(_) => carry.clear(),
            Err(error) if error.error_len().is_some() => return Ok(false),
            Err(error) => {
                carry.clear();
                carry.extend_from_slice(&candidate[error.valid_up_to()..]);
                if carry.len() > 3 {
                    return Ok(false);
                }
            }
        }
        if line_count < count {
            return Ok(carry.is_empty());
        }
    }
}

fn first_line_contains(path: &Path, needle: &[u8]) -> io::Result<bool> {
    let mut reader = File::open(path)?;
    let mut buffer = [0_u8; STREAM_BUFFER_SIZE];
    let mut overlap = Vec::with_capacity(needle.len().saturating_sub(1));
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            return Ok(overlap.windows(needle.len()).any(|window| window == needle));
        }
        let line_count = buffer[..count]
            .iter()
            .position(|byte| *byte == b'\n')
            .unwrap_or(count);
        let mut candidate = Vec::with_capacity(overlap.len() + line_count);
        candidate.extend_from_slice(&overlap);
        candidate.extend_from_slice(&buffer[..line_count]);
        if candidate
            .windows(needle.len())
            .any(|window| window == needle)
        {
            return Ok(true);
        }
        let retain = needle.len().saturating_sub(1).min(candidate.len());
        overlap.clear();
        overlap.extend_from_slice(&candidate[candidate.len() - retain..]);
        if line_count < count {
            return Ok(false);
        }
    }
}

fn rewrite_first_line(source: &Path, output: &Path, old: &[u8], new: &[u8]) -> io::Result<()> {
    let input = File::open(source)?;
    let mut reader = BufReader::with_capacity(STREAM_BUFFER_SIZE, input);
    let mut writer = BufWriter::with_capacity(STREAM_BUFFER_SIZE, File::create(output)?);
    let mut pending = Vec::with_capacity(old.len());
    let mut byte = [0_u8; 1];
    loop {
        let count = reader.read(&mut byte)?;
        if count == 0 {
            writer.write_all(&pending)?;
            break;
        }
        if byte[0] == b'\n' {
            writer.write_all(&pending)?;
            writer.write_all(&byte)?;
            io::copy(&mut reader, &mut writer)?;
            break;
        }
        pending.push(byte[0]);
        loop {
            if pending == old {
                writer.write_all(new)?;
                pending.clear();
                break;
            }
            if old.starts_with(&pending) {
                break;
            }
            writer.write_all(&pending[..1])?;
            pending.remove(0);
        }
    }
    writer.flush()
}

#[derive(Default)]
pub struct CompressManpagesPolicy;

impl CompressManpagesPolicy {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    fn is_manpage(path: &str) -> bool {
        const MAN_DIRS: &[&str] = &[
            "/man/", "/man1/", "/man2/", "/man3/", "/man4/", "/man5/", "/man6/", "/man7/", "/man8/",
        ];
        const MAN_EXTS: &[&str] = &[".1", ".2", ".3", ".4", ".5", ".6", ".7", ".8", ".n", ".l"];
        MAN_DIRS.iter().any(|directory| path.contains(directory))
            && path
                .rsplit('/')
                .next()
                .is_some_and(|name| MAN_EXTS.iter().any(|extension| name.ends_with(extension)))
    }
}

impl BuildPolicy for CompressManpagesPolicy {
    fn name(&self) -> &str {
        "CompressManpages"
    }

    fn apply(&self, ctx: &mut PolicyContext<'_>) -> Result<PolicyAction> {
        if !ctx.entry.node.kind.is_regular()
            || !Self::is_manpage(&ctx.entry.path)
            || has_prefix(ctx.source_path, &[0x1f, 0x8b])?
        {
            return Ok(PolicyAction::Keep);
        }
        let mut input = File::open(ctx.source_path)?;
        let output = File::create(ctx.replacement_path)?;
        let mut encoder = flate2::write::GzEncoder::new(output, flate2::Compression::default());
        io::copy(&mut input, &mut encoder)
            .map_err(|error| PolicyError::Execution(format!("Failed to compress: {error}")))?;
        encoder.finish().map_err(|error| {
            PolicyError::Execution(format!("Failed to finish compression: {error}"))
        })?;
        Ok(PolicyAction::Replace(ctx.replacement_path.to_path_buf()))
    }
}

#[cfg(test)]
pub(super) fn strip_elf_for_test(source: &Path, output: &Path) -> Result<u64, String> {
    strip_elf_streaming(source, output)
}
