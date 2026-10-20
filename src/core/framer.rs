//! Optional framing of the device stream before it goes to clients.

use bytes::{BufMut, Bytes, BytesMut};

use crate::config::FrameMode;

/// A frame that grows past this without a terminator is sent as it is.
pub const MAX_FRAME: usize = 64 * 1024;

pub struct Framer {
    mode: FrameMode,
    pending: BytesMut,
}

impl Framer {
    pub fn new(mode: FrameMode) -> Self {
        Framer {
            mode,
            pending: BytesMut::new(),
        }
    }

    /// Frames ready to send after `chunk`.
    pub fn push(&mut self, chunk: Bytes, out: &mut Vec<Bytes>) {
        let delim = match self.mode {
            FrameMode::None => return out.push(chunk),
            FrameMode::LengthPrefix => {
                for part in chunk.chunks(u16::MAX as usize) {
                    let mut f = BytesMut::with_capacity(part.len() + 2);
                    f.put_u16(part.len() as u16);
                    f.extend_from_slice(part);
                    out.push(f.freeze());
                }
                return;
            }
            FrameMode::Line => b'\n',
            FrameMode::NullTerminated => 0,
        };
        self.pending.extend_from_slice(&chunk);
        while let Some(i) = self.pending.iter().position(|&b| b == delim) {
            out.push(self.pending.split_to(i + 1).freeze());
        }
        if self.pending.len() > MAX_FRAME {
            out.push(self.pending.split().freeze());
        }
    }

    /// What is left when the source closes.
    pub fn flush(&mut self) -> Option<Bytes> {
        (!self.pending.is_empty()).then(|| self.pending.split().freeze())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(mode: FrameMode, chunks: &[&[u8]]) -> Vec<Vec<u8>> {
        let mut f = Framer::new(mode);
        let mut out = Vec::new();
        for c in chunks {
            f.push(Bytes::copy_from_slice(c), &mut out);
        }
        out.into_iter().map(|b| b.to_vec()).collect()
    }

    #[test]
    fn none_passes_chunks_through() {
        assert_eq!(
            run(FrameMode::None, &[b"ab", b"c"]),
            vec![b"ab".to_vec(), b"c".to_vec()]
        );
    }

    #[test]
    fn line_mode_joins_partial_nmea_sentences() {
        let out = run(
            FrameMode::Line,
            &[b"$GPGGA,1", b"23*47\r\n$GPRMC", b",x*00\r\n"],
        );
        assert_eq!(
            out,
            vec![b"$GPGGA,123*47\r\n".to_vec(), b"$GPRMC,x*00\r\n".to_vec()]
        );
    }

    #[test]
    fn null_terminated_and_flush() {
        let mut f = Framer::new(FrameMode::NullTerminated);
        let mut out = Vec::new();
        f.push(Bytes::from_static(b"\x02a\x00\x03b"), &mut out);
        assert_eq!(out, vec![Bytes::from_static(b"\x02a\x00")]);
        assert_eq!(f.flush(), Some(Bytes::from_static(b"\x03b")));
        assert_eq!(f.flush(), None);
    }

    #[test]
    fn length_prefix_is_big_endian() {
        assert_eq!(
            run(FrameMode::LengthPrefix, &[b"\x10\x20\x30"]),
            vec![vec![0, 3, 0x10, 0x20, 0x30]]
        );
    }

    #[test]
    fn runaway_line_is_bounded() {
        let big = vec![b'x'; MAX_FRAME + 1];
        let out = run(FrameMode::Line, &[&big]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].len(), MAX_FRAME + 1);
    }
}
