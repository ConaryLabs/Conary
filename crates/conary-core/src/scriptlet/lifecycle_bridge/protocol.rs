// conary-core/src/scriptlet/lifecycle_bridge/protocol.rs

//! Version 1 wire grammar:
//! `CONARY-LIFECYCLE-BRIDGE 1 <argc>\n`, followed by one command and `argc`
//! arguments, each encoded as `<byte-count>\n<raw-bytes>`. Responses carry an
//! exact 0..255 status and octal-escaped stdout/stderr lengths and lines. Clean
//! EOF between frames ends the session; EOF, timeout, or malformed data inside
//! a frame fails the lifecycle bridge.

use super::{LifecycleBridgeHandler, LifecycleBridgeRequest, LifecycleBridgeResponse};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const MAGIC: &[u8] = b"CONARY-LIFECYCLE-BRIDGE";
const VERSION: &[u8] = b"1";
const MAX_HEADER_BYTES: usize = 256;
const MAX_ARGUMENTS: usize = 256;
const MAX_VALUE_BYTES: usize = 256 * 1024;
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;

pub(super) const SHELL_LIBRARY: &str = include_str!("shell_library.inc");

pub(super) const EXECUTABLE_SHIM: &str = concat!(
    "#!/bin/sh\n",
    include_str!("shell_library.inc"),
    "\n_conary_bridge_exchange \"$0\" \"$@\" || exit $?\n",
    "_conary_bridge_emit_response\n",
    "exit $?\n",
);

pub(super) fn serve(
    stream: UnixStream,
    handler: Arc<dyn LifecycleBridgeHandler>,
    timeout: Duration,
    stop: &AtomicBool,
) -> std::result::Result<(), String> {
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|error| format!("cannot set lifecycle bridge read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|error| format!("cannot set lifecycle bridge write timeout: {error}"))?;
    let reader_stream = stream
        .try_clone()
        .map_err(|error| format!("cannot clone lifecycle bridge stream: {error}"))?;
    let mut reader = BufReader::new(reader_stream);
    let mut writer = stream;

    loop {
        let header = match read_header(&mut reader) {
            Ok(Some(header)) => header,
            Ok(None) => return Ok(()),
            Err(error) if stop.load(Ordering::Acquire) && is_shutdown_error(&error) => {
                return Ok(());
            }
            Err(error) if is_idle_timeout(&error) => {
                if stop.load(Ordering::Acquire) {
                    return Ok(());
                }
                continue;
            }
            Err(error) => return Err(format!("lifecycle bridge request header failed: {error}")),
        };
        let argument_count = parse_request_header(&header)?;
        let request = read_request(&mut reader, argument_count)
            .map_err(|error| format!("lifecycle bridge request body failed: {error}"))?;
        let response = handler
            .handle(&request)
            .map_err(|error| format!("lifecycle bridge handler failed: {error}"))?;
        write_response(&mut writer, &response)
            .map_err(|error| format!("lifecycle bridge response failed: {error}"))?;
    }
}

fn read_header(reader: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    let read = match reader
        .take((MAX_HEADER_BYTES + 1) as u64)
        .read_until(b'\n', &mut line)
    {
        Ok(read) => read,
        Err(error) if line.is_empty() => return Err(error),
        Err(error) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("header read stopped after {} bytes: {error}", line.len()),
            ));
        }
    };
    if read == 0 {
        return Ok(None);
    }
    if line.len() > MAX_HEADER_BYTES || !line.ends_with(b"\n") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "header exceeds its bound or lacks a newline",
        ));
    }
    line.pop();
    Ok(Some(line))
}

fn parse_request_header(header: &[u8]) -> std::result::Result<usize, String> {
    let fields = header.split(|byte| *byte == b' ').collect::<Vec<_>>();
    if fields.len() != 3 || fields[0] != MAGIC || fields[1] != VERSION {
        return Err("unsupported lifecycle bridge request header".to_string());
    }
    let argument_count = parse_decimal(fields[2], "argument count")?;
    if argument_count > MAX_ARGUMENTS {
        return Err(format!(
            "lifecycle bridge argument count {argument_count} exceeds {MAX_ARGUMENTS}"
        ));
    }
    Ok(argument_count)
}

fn read_request(
    reader: &mut impl BufRead,
    argument_count: usize,
) -> io::Result<LifecycleBridgeRequest> {
    let command = read_value(reader)?;
    if command.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "command identity is empty",
        ));
    }
    let mut total = command.len();
    let mut arguments = Vec::with_capacity(argument_count);
    for _ in 0..argument_count {
        let value = read_value(reader)?;
        total = total
            .checked_add(value.len())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "request length overflow"))?;
        if total > MAX_REQUEST_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request exceeds its byte bound",
            ));
        }
        arguments.push(value);
    }
    Ok(LifecycleBridgeRequest { command, arguments })
}

fn read_value(reader: &mut impl BufRead) -> io::Result<Vec<u8>> {
    let length_line = read_required_header(reader, "value length")?;
    let length = parse_decimal(&length_line, "value length")
        .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))?;
    if length > MAX_VALUE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("value length {length} exceeds {MAX_VALUE_BYTES}"),
        ));
    }
    let mut value = vec![0; length];
    reader.read_exact(&mut value)?;
    Ok(value)
}

fn read_required_header(reader: &mut impl BufRead, label: &str) -> io::Result<Vec<u8>> {
    match read_header(reader)? {
        Some(line) => Ok(line),
        None => Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!("EOF before {label}"),
        )),
    }
}

fn write_response(writer: &mut impl Write, response: &LifecycleBridgeResponse) -> io::Result<()> {
    if response.stdout.len() > MAX_OUTPUT_BYTES || response.stderr.len() > MAX_OUTPUT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "handler response exceeds its output bound",
        ));
    }
    let stdout = encode_octal(&response.stdout);
    let stderr = encode_octal(&response.stderr);
    writeln!(
        writer,
        "{} {} {} {} {}",
        std::str::from_utf8(MAGIC).expect("bridge magic is ASCII"),
        std::str::from_utf8(VERSION).expect("bridge version is ASCII"),
        response.exit_code,
        stdout.len(),
        stderr.len()
    )?;
    writer.write_all(&stdout)?;
    writer.write_all(b"\n")?;
    writer.write_all(&stderr)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

fn encode_octal(bytes: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(bytes.len() * 5);
    for byte in bytes {
        encoded.extend_from_slice(b"\\0");
        encoded.push(b'0' + ((byte >> 6) & 0o3));
        encoded.push(b'0' + ((byte >> 3) & 0o7));
        encoded.push(b'0' + (byte & 0o7));
    }
    encoded
}

fn parse_decimal(bytes: &[u8], label: &str) -> std::result::Result<usize, String> {
    if bytes.is_empty() || bytes.iter().any(|byte| !byte.is_ascii_digit()) {
        return Err(format!("lifecycle bridge {label} is not decimal"));
    }
    if bytes.len() > 1 && bytes[0] == b'0' {
        return Err(format!("lifecycle bridge {label} has a leading zero"));
    }
    let text =
        std::str::from_utf8(bytes).map_err(|_| format!("lifecycle bridge {label} is not ASCII"))?;
    text.parse()
        .map_err(|_| format!("lifecycle bridge {label} is out of range"))
}

fn is_idle_timeout(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

fn is_shutdown_error(error: &io::Error) -> bool {
    is_idle_timeout(error)
        || matches!(
            error.kind(),
            io::ErrorKind::UnexpectedEof
                | io::ErrorKind::ConnectionReset
                | io::ErrorKind::BrokenPipe
                | io::ErrorKind::NotConnected
        )
}
