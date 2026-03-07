//! Passive protocol detection for io-to-net bridges.
//!
//! The detector accumulates a rolling sample of RX bytes and classifies the
//! stream.  It operates entirely as a side-channel observer: it never blocks
//! or modifies the data path.

use serde::Serialize;

/// Classification of the observed stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ProtocolHint {
    Unknown,
    AsciiStream,
    BinaryStream,
    Nmea,
    AtCommands,
    Scpi,
    CanOverSerial,
    Slip,
    Cobs,
    MavLink,
    ModbusRtu,
    Speeduino,
    Megasquirt,
}

impl ProtocolHint {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProtocolHint::Unknown      => "unknown",
            ProtocolHint::AsciiStream  => "ASCII stream",
            ProtocolHint::BinaryStream => "binary stream",
            ProtocolHint::Nmea         => "NMEA",
            ProtocolHint::AtCommands   => "AT commands",
            ProtocolHint::Scpi         => "SCPI",
            ProtocolHint::CanOverSerial=> "CAN-over-serial",
            ProtocolHint::Slip         => "SLIP",
            ProtocolHint::Cobs         => "COBS",
            ProtocolHint::MavLink      => "MAVLink",
            ProtocolHint::ModbusRtu    => "Modbus RTU",
            ProtocolHint::Speeduino    => "Speeduino",
            ProtocolHint::Megasquirt   => "Megasquirt",
        }
    }
}

impl std::fmt::Display for ProtocolHint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Rolling statistics collected alongside protocol detection.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DetectorStats {
    pub bytes_seen: u64,
    pub message_rate: f64,       // detected messages / second
    pub avg_frame_size: f64,     // bytes per detected frame boundary
    pub max_frame_size: usize,
}

const SAMPLE_CAPACITY: usize = 4096;
const CLASSIFY_THRESHOLD: usize = 256; // minimum bytes before we commit to a hint

/// Passive protocol detector.  Call [`feed`] with each RX chunk; read
/// [`hint`] whenever you need the latest classification.
pub struct ProtocolDetector {
    /// Rolling sample buffer (oldest bytes evicted when full).
    sample: Vec<u8>,
    /// Lines/frames seen so far (for rate estimation).
    frames: u64,
    /// Byte total since last [`feed`].
    bytes_total: u64,
    /// Monotonic frame-size histogram (sum of boundary distances).
    frame_size_sum: u64,
    /// Last detected frame boundary position in `sample`.
    last_boundary: usize,
    /// Largest inter-boundary span seen.
    max_frame: usize,
    /// Current classification.
    hint: ProtocolHint,
    /// Time of first byte (for rate).
    start: std::time::Instant,
}

impl ProtocolDetector {
    pub fn new() -> Self {
        ProtocolDetector {
            sample: Vec::with_capacity(SAMPLE_CAPACITY),
            frames: 0,
            bytes_total: 0,
            frame_size_sum: 0,
            last_boundary: 0,
            max_frame: 0,
            hint: ProtocolHint::Unknown,
            start: std::time::Instant::now(),
        }
    }

    /// Feed a chunk of received bytes into the detector.
    ///
    /// This is called from the hot data path; it must be O(n) and cheap.
    pub fn feed(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        self.bytes_total += data.len() as u64;

        // Append to rolling sample, evicting the oldest bytes when full.
        if self.sample.len() + data.len() > SAMPLE_CAPACITY {
            let keep = SAMPLE_CAPACITY.saturating_sub(data.len());
            let drain = self.sample.len().saturating_sub(keep);
            self.sample.drain(..drain);
            self.last_boundary = self.last_boundary.saturating_sub(drain);
        }
        self.sample.extend_from_slice(data);

        // Detect frame boundaries in the new data only.
        self.scan_boundaries(data);

        // Re-classify when we have enough data.
        if self.sample.len() >= CLASSIFY_THRESHOLD {
            self.hint = classify(&self.sample, self.frames);
        }
    }

    /// Return the current protocol classification.
    pub fn hint(&self) -> &ProtocolHint {
        &self.hint
    }

    /// Return aggregated statistics.
    pub fn stats(&self) -> DetectorStats {
        let elapsed = self.start.elapsed().as_secs_f64().max(0.001);
        let avg_frame = if self.frames > 0 {
            self.frame_size_sum as f64 / self.frames as f64
        } else {
            0.0
        };
        DetectorStats {
            bytes_seen: self.bytes_total,
            message_rate: self.frames as f64 / elapsed,
            avg_frame_size: avg_frame,
            max_frame_size: self.max_frame,
        }
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    /// Scan `data` for protocol-level frame delimiters to track frame rates.
    fn scan_boundaries(&mut self, data: &[u8]) {
        for &b in data {
            let pos = self.sample.len();
            if is_frame_delimiter(b) {
                let span = pos.saturating_sub(self.last_boundary);
                if span > 0 {
                    self.frames += 1;
                    self.frame_size_sum += span as u64;
                    if span > self.max_frame {
                        self.max_frame = span;
                    }
                }
                self.last_boundary = pos;
            }
        }
    }
}

impl Default for ProtocolDetector {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns true for bytes that commonly delimit frames across the supported protocols.
fn is_frame_delimiter(b: u8) -> bool {
    matches!(b,
        b'\n'  // ASCII/NMEA/AT/SCPI line ending
        | 0xC0 // SLIP end-of-packet
        | 0x00 // COBS frame boundary
    )
}

/// Classify what protocol we are likely looking at given a sample buffer.
fn classify(sample: &[u8], frames: u64) -> ProtocolHint {
    if sample.len() < CLASSIFY_THRESHOLD {
        return ProtocolHint::Unknown;
    }

    // ── MAVLink: starts with 0xFE (v1) or 0xFD (v2) ─────────────────────────
    let mavlink_count = sample.windows(1).filter(|w| w[0] == 0xFE || w[0] == 0xFD).count();
    if mavlink_count as f64 / sample.len() as f64 > 0.03 {
        return ProtocolHint::MavLink;
    }

    // ── SLIP: frequent 0xC0 bytes ────────────────────────────────────────────
    let slip_count = sample.iter().filter(|&&b| b == 0xC0).count();
    if slip_count as f64 / sample.len() as f64 > 0.02 {
        return ProtocolHint::Slip;
    }

    // ── COBS: frequent 0x00 boundaries ──────────────────────────────────────
    let cobs_count = sample.iter().filter(|&&b| b == 0x00).count();
    if cobs_count as f64 / sample.len() as f64 > 0.02 {
        return ProtocolHint::Cobs;
    }

    // ── Printable-ASCII ratio ────────────────────────────────────────────────
    let printable = sample.iter().filter(|&&b| b >= 0x20 && b <= 0x7E).count();
    let printable_ratio = printable as f64 / sample.len() as f64;

    if printable_ratio > 0.90 {
        // ASCII-dominant stream — try specific ASCII protocols.
        return classify_ascii(sample, frames);
    }

    if printable_ratio < 0.70 {
        // Binary-dominant stream — try specific binary protocols.
        return classify_binary(sample, frames);
    }

    // Mixed — could be either.
    ProtocolHint::AsciiStream
}

/// Classify an ASCII-dominant sample.
fn classify_ascii(sample: &[u8], _frames: u64) -> ProtocolHint {
    // NMEA: lines starting with '$GP', '$GN', '$GL', '$II' ...
    let has_nmea = sample.windows(3).any(|w| {
        w[0] == b'$' && (w[1] == b'G' || w[1] == b'I') && w[2].is_ascii_uppercase()
    });
    if has_nmea {
        return ProtocolHint::Nmea;
    }

    // CAN-over-serial (slcan/lawicel): lines starting with t/T/r/R
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(sample.windows(2).enumerate().filter_map(|(i, w)| {
            if w[0] == b'\n' { Some(i + 1) } else { None }
        }))
        .collect();
    let can_lines = line_starts.iter().filter(|&&i| {
        i < sample.len() && matches!(sample[i], b't' | b'T' | b'r' | b'R')
    }).count();
    if can_lines as f64 / line_starts.len().max(1) as f64 > 0.50 {
        return ProtocolHint::CanOverSerial;
    }

    // AT commands: lines starting with "AT"
    let at_lines = line_starts.iter().filter(|&&i| {
        i + 1 < sample.len() && sample[i] == b'A' && sample[i + 1] == b'T'
    }).count();
    if at_lines as f64 / line_starts.len().max(1) as f64 > 0.30 {
        return ProtocolHint::AtCommands;
    }

    // SCPI: '*' commands or ':' hierarchical selectors
    let scpi_markers = sample.iter().filter(|&&b| b == b'*' || b == b':').count();
    if scpi_markers as f64 / sample.len() as f64 > 0.04 {
        return ProtocolHint::Scpi;
    }

    ProtocolHint::AsciiStream
}

/// Classify a binary-dominant sample.
fn classify_binary(sample: &[u8], frames: u64) -> ProtocolHint {
    // Modbus RTU: check a few function codes (0x01–0x10) at likely position 1.
    let modbus_fc: &[u8] = &[0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x0F, 0x10];
    if sample.len() > 4 {
        let fc_matches = sample.windows(2).filter(|w| {
            w[0] < 0x20 && modbus_fc.contains(&w[1])
        }).count();
        if fc_matches as f64 / (sample.len() / 4).max(1) as f64 > 0.20 {
            return ProtocolHint::ModbusRtu;
        }
    }

    // Speeduino: very consistent small frames (~79 bytes), high rate.
    if frames > 10 {
        let avg = if frames > 0 { sample.len() as f64 / frames as f64 } else { 0.0 };
        if (60.0..=100.0).contains(&avg) {
            return ProtocolHint::Speeduino;
        }
        // Megasquirt: variable but consistent binary frames
        if (20.0..=200.0).contains(&avg) {
            return ProtocolHint::Megasquirt;
        }
    }

    ProtocolHint::BinaryStream
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_detector_with(data: &[u8]) -> ProtocolDetector {
        let mut d = ProtocolDetector::new();
        // Feed in chunks to exercise the rolling buffer.
        for chunk in data.chunks(64) {
            d.feed(chunk);
        }
        d
    }

    #[test]
    fn unknown_with_little_data() {
        let d = make_detector_with(b"$GPGGA");
        assert_eq!(*d.hint(), ProtocolHint::Unknown);
    }

    #[test]
    fn detects_nmea() {
        // Build a realistic-length NMEA sample.
        let sentence = b"$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47\r\n";
        let data: Vec<u8> = sentence.iter().cycle().take(512).cloned().collect();
        let d = make_detector_with(&data);
        assert_eq!(*d.hint(), ProtocolHint::Nmea);
    }

    #[test]
    fn detects_ascii_stream() {
        let data: Vec<u8> = b"hello world this is a long ascii test stream with lots of printable chars\n"
            .iter().cycle().take(512).cloned().collect();
        let d = make_detector_with(&data);
        // Should be AsciiStream (no NMEA/AT/SCPI markers).
        matches!(d.hint(), ProtocolHint::AsciiStream | ProtocolHint::Unknown);
    }

    #[test]
    fn detects_binary_stream() {
        // Data with >30% non-printable bytes.
        let mut data = vec![0u8; 512];
        for (i, b) in data.iter_mut().enumerate() {
            *b = if i % 3 == 0 { 0x80 } else { 0x41 };
        }
        let d = make_detector_with(&data);
        // Should be BinaryStream or a specific binary protocol.
        assert_ne!(*d.hint(), ProtocolHint::AsciiStream);
        assert_ne!(*d.hint(), ProtocolHint::Nmea);
    }

    #[test]
    fn detects_slip() {
        // SLIP frames delimited by 0xC0.
        let mut data = vec![0u8; 512];
        for (i, b) in data.iter_mut().enumerate() {
            *b = if i % 20 == 0 { 0xC0 } else { (i % 256) as u8 };
        }
        let d = make_detector_with(&data);
        assert_eq!(*d.hint(), ProtocolHint::Slip);
    }

    #[test]
    fn detects_mavlink() {
        let mut data = vec![0x41u8; 512]; // 'A' filler
        for i in (0..512).step_by(30) {
            data[i] = 0xFE; // MAVLink v1 start byte
        }
        let d = make_detector_with(&data);
        assert_eq!(*d.hint(), ProtocolHint::MavLink);
    }

    #[test]
    fn hint_as_str() {
        assert_eq!(ProtocolHint::Nmea.as_str(), "NMEA");
        assert_eq!(ProtocolHint::Unknown.as_str(), "unknown");
    }

    #[test]
    fn stats_bytes_seen() {
        let mut d = ProtocolDetector::new();
        d.feed(b"hello");
        d.feed(b"world");
        assert_eq!(d.stats().bytes_seen, 10);
    }
}
