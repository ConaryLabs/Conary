// conary-core/src/container/child_safety.rs

//! Async-signal-safe primitives for the window between `fork()` and `exec()`.
//!
//! `fork()` copies the whole address space but only the calling thread. Every
//! lock another thread held at that instant is inherited locked, with no owner
//! left to release it. glibc protects its own primitives across fork -- malloc
//! arenas and stdio `FILE` locks are re-initialized in the child -- so
//! allocation is survivable. Rust-level locks are not protected: `tracing`'s
//! global dispatcher is an `RwLock`, and a child that logs while another thread
//! held that lock at fork time blocks forever. The parent then converts the
//! stall into `Script timed out after ...`.
//!
//! So the child gets diagnostics that touch no lock and no allocator: a direct
//! `write(2)` of caller-owned bytes. Callers pass already-owned slices --
//! string literals, `OsStr` bytes from a path that exists, integers rendered
//! into a caller-supplied stack buffer.
//!
//! Anything added to the post-fork path must hold to this. `child_fork_safety`
//! in the container tests fails the build if a logging macro or a subprocess
//! spawn reappears in the child sources.

use std::os::fd::RawFd;

/// The inherited stderr. After the child's `dup2`, this is the pipe the parent
/// drains, so these diagnostics reach the caller as scriptlet stderr.
const CHILD_STDERR: RawFd = 2;

/// Every diagnostic is prefixed, because it lands in the sandboxed program's
/// own stderr stream and must be attributable.
const PREFIX: &[u8] = b"conary-sandbox: ";

/// Write one diagnostic line to the inherited stderr.
///
/// Allocation-free and lock-free: the parts are already-owned bytes and go out
/// in a single `write(2)` per segment. Short writes and errors are ignored --
/// there is no recovery available to a child that cannot report, and retrying
/// risks blocking on a full pipe the parent is no longer reading.
pub(in crate::container) fn child_diag(parts: &[&[u8]]) {
    write_all(PREFIX);
    for part in parts {
        write_all(part);
    }
    write_all(b"\n");
}

fn write_all(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    // SAFETY: `write(2)` is async-signal-safe. The pointer and length come from
    // a live borrow, and the return value is deliberately discarded.
    unsafe {
        libc::write(
            CHILD_STDERR,
            bytes.as_ptr() as *const libc::c_void,
            bytes.len(),
        );
    }
}

/// A stack buffer wide enough for any `i64` rendered by [`format_int`].
pub(in crate::container) type IntBuffer = [u8; 24];

/// An empty [`IntBuffer`], for callers that need one at a `let` binding.
pub(in crate::container) const fn int_buffer() -> IntBuffer {
    [0; 24]
}

/// Render `value` into `buffer` without allocating, returning the digits.
///
/// The child cannot use `format!`, so integer diagnostics -- errno values,
/// exit codes -- are rendered here into caller-owned storage.
pub(in crate::container) fn format_int(buffer: &mut IntBuffer, value: i64) -> &[u8] {
    if value == 0 {
        buffer[0] = b'0';
        return &buffer[..1];
    }
    let negative = value < 0;
    // Accumulate magnitude in i128 so i64::MIN negates without overflowing.
    let mut magnitude = (value as i128).unsigned_abs();
    let mut end = buffer.len();
    while magnitude > 0 {
        end -= 1;
        buffer[end] = b'0' + (magnitude % 10) as u8;
        magnitude /= 10;
    }
    if negative {
        end -= 1;
        buffer[end] = b'-';
    }
    &buffer[end..]
}

// Kernel ABI for the loopback interface. These are stable `ioctl` numbers and
// flag bits; they are spelled out rather than taken from `libc` so the layout
// this code depends on is visible at the point of use.
const SIOCGIFFLAGS: libc::c_ulong = 0x8913;
const SIOCSIFFLAGS: libc::c_ulong = 0x8914;
const IFF_UP: libc::c_short = 0x1;
const IFF_RUNNING: libc::c_short = 0x40;

/// The prefix of `struct ifreq` this code touches.
///
/// `ifreq` is a 16-byte name followed by a union; only the `short` flags arm is
/// used here, and the tail is padded to the full 40-byte size the kernel
/// expects on 64-bit Linux.
#[repr(C)]
struct IfReqFlags {
    name: [u8; 16],
    flags: libc::c_short,
    _padding: [u8; 22],
}

/// Bring the loopback interface up inside a fresh network namespace.
///
/// This replaces spawning `ip link set lo up`. Creating a subprocess between
/// `fork()` and `exec()` runs the whole `std::process` machinery in a child
/// with inherited lock state; two `ioctl` calls do the same job with no
/// allocation, no subprocess, and no dependency on `iproute2` being installed
/// in the sandbox.
///
/// Returns the failing errno, so the caller can report without allocating.
pub(in crate::container) fn bring_loopback_up() -> std::result::Result<(), i32> {
    // SAFETY: every call below is a raw syscall on a locally owned descriptor
    // and a zeroed, correctly sized `ifreq`.
    unsafe {
        let fd = libc::socket(
            libc::AF_INET,
            libc::SOCK_DGRAM | libc::SOCK_CLOEXEC,
            libc::IPPROTO_IP,
        );
        if fd < 0 {
            return Err(errno());
        }

        let mut request = IfReqFlags {
            name: [0; 16],
            flags: 0,
            _padding: [0; 22],
        };
        request.name[0] = b'l';
        request.name[1] = b'o';

        if libc::ioctl(fd, SIOCGIFFLAGS, &raw mut request) < 0 {
            let error = errno();
            libc::close(fd);
            return Err(error);
        }

        request.flags |= IFF_UP | IFF_RUNNING;

        let result = if libc::ioctl(fd, SIOCSIFFLAGS, &raw const request) < 0 {
            Err(errno())
        } else {
            Ok(())
        };
        libc::close(fd);
        result
    }
}

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(-1)
}

/// Set a resource limit if the value is non-zero.
///
/// Lives here because it runs only in the forked child: it reports through
/// [`child_diag`], and keeping it in this module puts it under the same
/// fork-safety check as the rest of the post-fork path.
pub(in crate::container) fn set_rlimit(resource: super::RlimitResource, value: u64, name: &str) {
    if value == 0 {
        return;
    }
    let limit = libc::rlimit {
        rlim_cur: value,
        rlim_max: value,
    };
    if let Err(err) = super::set_rlimit_syscall(resource, &limit) {
        let mut digits = int_buffer();
        child_diag(&[
            b"setrlimit ",
            name.as_bytes(),
            b" failed: errno ",
            format_int(&mut digits, err.raw_os_error().unwrap_or(-1) as i64),
        ]);
    }
}
