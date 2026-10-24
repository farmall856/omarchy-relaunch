//! Newline-framed input, byte-bounded WHILE reading (Sol #11).
//!
//! A frame is the bytes before a `\n`. The limit is enforced as bytes arrive,
//! so an unterminated or whitespace-padded line cannot grow an unbounded buffer
//! before any decoded-size check runs. An unterminated tail at EOF is not a
//! frame (Sol #13): EOF is reported as `Eof`, never as a short final frame.

use tokio::io::{AsyncBufRead, AsyncBufReadExt};

/// One unit of newline-framed input.
#[derive(Debug, PartialEq, Eq)]
pub enum Frame {
    /// A complete frame, without its newline.
    Line(Vec<u8>),
    /// End of input, including EOF in the middle of an unterminated line.
    Eof,
    /// The frame exceeded the limit. Reading stops at that point.
    TooLong,
}

/// Read one frame of at most `max` bytes (excluding the newline).
///
/// # Errors
///
/// Only a read error from `r`.
pub async fn read_frame<R: AsyncBufRead + Unpin>(r: &mut R, max: usize) -> std::io::Result<Frame> {
    let mut buf = Vec::new();
    loop {
        let avail = r.fill_buf().await?;
        if avail.is_empty() {
            return Ok(Frame::Eof);
        }
        if let Some(i) = avail.iter().position(|&b| b == b'\n') {
            if buf.len() + i > max {
                r.consume(i + 1);
                return Ok(Frame::TooLong);
            }
            buf.extend(avail.iter().take(i));
            r.consume(i + 1);
            return Ok(Frame::Line(buf));
        }
        let n = avail.len();
        if buf.len() + n > max {
            r.consume(n);
            return Ok(Frame::TooLong);
        }
        buf.extend_from_slice(avail);
        r.consume(n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::BufReader;

    async fn frames(input: &[u8], max: usize) -> Vec<Frame> {
        let mut r = BufReader::with_capacity(4, input);
        let mut out = Vec::new();
        loop {
            let f = read_frame(&mut r, max).await.unwrap();
            let stop = !matches!(f, Frame::Line(_));
            out.push(f);
            if stop {
                return out;
            }
        }
    }

    #[tokio::test]
    async fn splits_on_newline_across_small_buffer_fills() {
        assert_eq!(
            frames(b"abcdef\nxy\n", 16).await,
            vec![
                Frame::Line(b"abcdef".to_vec()),
                Frame::Line(b"xy".to_vec()),
                Frame::Eof
            ]
        );
    }

    #[tokio::test]
    async fn unterminated_tail_is_eof_not_a_frame() {
        assert_eq!(
            frames(br#"{"decision":"authorize"}"#, 256).await,
            vec![Frame::Eof]
        );
    }

    #[tokio::test]
    async fn exactly_max_is_accepted_one_more_is_too_long() {
        assert_eq!(
            frames(b"12345678\n", 8).await,
            vec![Frame::Line(b"12345678".to_vec()), Frame::Eof]
        );
        assert_eq!(frames(b"123456789\n", 8).await, vec![Frame::TooLong]);
    }

    #[tokio::test]
    async fn oversize_unterminated_input_stops_at_the_limit() {
        // Whitespace padding with no newline: refused once it passes the limit.
        let padded = vec![b' '; 10_000];
        assert_eq!(frames(&padded, 64).await, vec![Frame::TooLong]);
    }
}
