//! USB / serial adapter framework, CDC-ACM part (F-ACQ-04): port discovery, read-only ports and
//! framing. Per-device decoders plug in on top of [`Framer`] frames.
//!
//! A port is handed out only as [`ReadOnlyPort`], which implements `Read` and nothing else, so no
//! adapter can write to a serial device by accident. Poll requests a device ICD defines are
//! write exceptions and go through `docs/write-exceptions.md` (category b).
//!
//! Framing is bounded: a frame longer than its limit is discarded and the framer resynchronises
//! on the next delimiter; the discard is reported so device health counts it.

use std::io::Read;
use std::time::Duration;

/// Serial line settings, from the device's ICD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerialSettings {
    /// OS port name (`COM3`, `/dev/ttyACM0`).
    pub path: String,
    /// Baud rate.
    pub baud: u32,
    /// Read timeout; a read with no data returns after this long.
    pub read_timeout: Duration,
}

/// A discovered port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortInfo {
    /// OS port name.
    pub path: String,
    /// USB vendor and product IDs, when the port is a USB device.
    pub usb: Option<(u16, u16)>,
    /// USB serial number, when reported.
    pub serial_number: Option<String>,
}

/// Serial errors.
#[derive(Debug, thiserror::Error)]
pub enum SerialError {
    /// The OS refused or failed.
    #[error("serial port: {0}")]
    Port(String),
}

impl From<serialport::Error> for SerialError {
    fn from(e: serialport::Error) -> Self {
        SerialError::Port(e.to_string())
    }
}

/// Lists the serial ports the OS reports.
pub fn available_ports() -> Result<Vec<PortInfo>, SerialError> {
    Ok(serialport::available_ports()?
        .into_iter()
        .map(|p| {
            let (usb, serial_number) = match p.port_type {
                serialport::SerialPortType::UsbPort(u) => (Some((u.vid, u.pid)), u.serial_number),
                _ => (None, None),
            };
            PortInfo {
                path: p.port_name,
                usb,
                serial_number,
            }
        })
        .collect())
}

/// A serial port that can only be read.
pub struct ReadOnlyPort(Box<dyn serialport::SerialPort>);

impl Read for ReadOnlyPort {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }
}

/// Opens a port for reading (8 data bits, no parity, one stop bit unless the ICD says otherwise
/// in later adapters).
pub fn open_read_only(settings: &SerialSettings) -> Result<ReadOnlyPort, SerialError> {
    let port = serialport::new(&settings.path, settings.baud)
        .timeout(settings.read_timeout)
        .open()?;
    Ok(ReadOnlyPort(port))
}

/// How a device delimits its messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    /// Messages end with `delimiter` (not included in the frame), at most `max_len` bytes.
    Delimited {
        /// End-of-message byte (e.g. `\n`).
        delimiter: u8,
        /// Longest valid message.
        max_len: usize,
    },
}

/// Output of the framer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameEvent {
    /// A complete message.
    Frame(Vec<u8>),
    /// A message exceeded its limit and was discarded (count as dropped).
    Overflow,
}

/// Splits a byte stream into messages.
#[derive(Debug, Clone)]
pub struct Framer {
    framing: Framing,
    buf: Vec<u8>,
    discarding: bool,
}

impl Framer {
    /// A framer for `framing`.
    pub fn new(framing: Framing) -> Self {
        Self {
            framing,
            buf: Vec::new(),
            discarding: false,
        }
    }

    /// Feeds received bytes; returns the events they complete.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<FrameEvent> {
        let Framing::Delimited { delimiter, max_len } = self.framing;
        let mut out = Vec::new();
        for &b in bytes {
            if b == delimiter {
                if self.discarding {
                    self.discarding = false;
                } else {
                    out.push(FrameEvent::Frame(std::mem::take(&mut self.buf)));
                }
                continue;
            }
            if self.discarding {
                continue;
            }
            if self.buf.len() == max_len {
                self.buf.clear();
                self.discarding = true;
                out.push(FrameEvent::Overflow);
                continue;
            }
            self.buf.push(b);
        }
        out
    }

    /// Drops a partial message (after a link loss, its start is gone).
    pub fn reset(&mut self) {
        self.buf.clear();
        self.discarding = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framer() -> Framer {
        Framer::new(Framing::Delimited {
            delimiter: b'\n',
            max_len: 8,
        })
    }

    /// F-ACQ-04: messages split across reads are reassembled.
    #[test]
    fn f_acq_04_frames_are_reassembled_across_reads() {
        let mut f = framer();
        assert!(f.push(b"HR=7").is_empty());
        assert_eq!(f.push(b"2\nSP"), [FrameEvent::Frame(b"HR=72".to_vec())]);
        assert_eq!(f.push(b"O2=97\n"), [FrameEvent::Frame(b"SPO2=97".to_vec())]);
    }

    #[test]
    fn oversized_messages_are_discarded_and_the_framer_resynchronises() {
        let mut f = framer();
        assert_eq!(
            f.push(b"0123456789ABC\nOK\n"),
            [FrameEvent::Overflow, FrameEvent::Frame(b"OK".to_vec())]
        );
    }

    #[test]
    fn reset_drops_a_partial_message() {
        let mut f = framer();
        f.push(b"PARTI");
        f.reset();
        assert_eq!(f.push(b"AL\n"), [FrameEvent::Frame(b"AL".to_vec())]);
    }
}
