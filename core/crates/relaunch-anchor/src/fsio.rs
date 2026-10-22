//! Bounded, type-checked file reads for the login path (reviewer I3/M3).
//!
//! `emit-lua` runs synchronously while Hyprland evaluates its config, so a
//! config or launcher that is a FIFO, device, or enormous file must not be able
//! to block or exhaust memory. We open non-blocking, assert the *opened
//! descriptor* is a regular file, and read through a byte cap.

use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// 1 MiB is far above any real config/launcher.
pub const MAX_FILE_BYTES: usize = 1024 * 1024;

pub fn read_regular_capped(path: &Path, max: usize) -> Result<Vec<u8>, String> {
    // O_NONBLOCK so opening a FIFO with no writer returns instead of hanging;
    // O_CLOEXEC as hygiene. We do not add O_NOFOLLOW here: the launcher path is
    // canonicalized by the caller, and config.json may legitimately be a
    // dotfiles symlink. The S_ISREG check is on the opened fd.
    let f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    let md = f.metadata().map_err(|e| format!("fstat: {e}"))?;
    if !md.file_type().is_file() {
        return Err("not a regular file".into());
    }
    if md.len() as usize > max {
        return Err("file too large".into());
    }
    let mut buf = Vec::with_capacity(md.len() as usize);
    // Cap the read independently of the stat'd size (it can race).
    f.take(max as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("read: {e}"))?;
    if buf.len() > max {
        return Err("file too large".into());
    }
    Ok(buf)
}
