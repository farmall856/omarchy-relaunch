//! Running a system tool and reading its output, bounded (INV-5): the ONE
//! reader every binary uses for `hyprctl` and `date`, so no caller can read an
//! unbounded stream or hang on a child blocked writing past the cap.

use std::ffi::OsStr;
use std::io::Read;
use std::process::{Command, Stdio};

/// Most bytes of a tool's stdout read (`hyprctl clients -j` on a busy desktop
/// is a few hundred KiB).
pub const MAX_OUTPUT_BYTES: u64 = 16 * 1024 * 1024;

/// Run `program` with `args`, stdin and stderr closed, and return its stdout.
/// Output beyond [`MAX_OUTPUT_BYTES`] is refused and the child is killed.
///
/// # Errors
///
/// A message if the tool cannot be started, prints too much, or exits
/// unsuccessfully.
pub fn output_capped(program: impl AsRef<OsStr>, args: &[&str]) -> Result<Vec<u8>, String> {
    let program = program.as_ref();
    let name = program.to_string_lossy().into_owned();
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{name}: {e}"))?;
    let mut out = Vec::new();
    let read = child.stdout.take().map(|s| {
        s.take(MAX_OUTPUT_BYTES.saturating_add(1))
            .read_to_end(&mut out)
    });
    let too_large = u64::try_from(out.len()).map_or(true, |n| n > MAX_OUTPUT_BYTES);
    if too_large {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|e| format!("{name}: {e}"))?;
    if too_large {
        return Err(format!("{name}: output exceeds {MAX_OUTPUT_BYTES} bytes"));
    }
    if !status.success() || !matches!(read, Some(Ok(_))) {
        return Err(format!("{name}: command failed"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdout_is_returned() {
        assert_eq!(
            output_capped("/bin/sh", &["-c", "printf hi"]).unwrap(),
            b"hi"
        );
    }

    #[test]
    fn a_failing_tool_is_an_error() {
        assert!(output_capped("/bin/sh", &["-c", "printf hi; exit 3"]).is_err());
        assert!(output_capped("/nonexistent/tool", &[]).is_err());
    }

    #[test]
    fn output_past_the_cap_is_refused_and_does_not_hang() {
        // `yes` never stops on its own: the reader must kill it, not wait.
        let err = output_capped("/usr/bin/yes", &[]).unwrap_err();
        assert!(err.contains("exceeds"), "{err}");
        let exact = format!("head -c {MAX_OUTPUT_BYTES} /dev/zero");
        assert_eq!(
            output_capped("/bin/sh", &["-c", &exact]).unwrap().len(),
            usize::try_from(MAX_OUTPUT_BYTES).unwrap()
        );
    }
}
