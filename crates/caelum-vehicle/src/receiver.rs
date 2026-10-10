//! The thin async UDP receiver around [`Decoder`] (F-GND-11 subscribe side, R-INT-04).
//!
//! Receive only (SR-011, R-SEC-04): [`VehicleReceiver`] owns its socket privately and its whole
//! public API is `bind`, `local_addr`, `recv`, `stats`, `correlation` and `config`. It never
//! hands the socket out, never connects it and has no method that sends, writes or commands.
//! It does not join SBN peering either: SBN subscription and heartbeat traffic would be outbound,
//! so the vehicle side forwards the configured MIDs statically (CR-015 §2).

use std::io;
use std::net::SocketAddr;

use caelum_domain::time::{LeapSeconds, MetEpoch, TaiNs};
use tokio::net::UdpSocket;

use crate::config::VehicleConfig;
use crate::decode::{Decoded, Decoder, Stats};
use crate::time::CorrelationStatus;

/// Largest UDP payload; a full buffer means no datagram is ever truncated.
const MAX_DATAGRAM: usize = 65_535;

/// Why `recv` produced no decode.
#[derive(Debug, thiserror::Error)]
pub enum RecvError {
    /// Socket error.
    #[error("vehicle link: {0}")]
    Io(#[from] io::Error),
    /// The station clock gave no time, so the datagram could not be stamped (dropped, counted).
    #[error("station clock unavailable; datagram dropped")]
    NoStationTime,
}

/// Receives cFS telemetry datagrams and decodes them. `C` is the station TAI clock.
pub struct VehicleReceiver<C> {
    socket: UdpSocket,
    decoder: Decoder,
    clock: C,
    buf: Vec<u8>,
    no_station_time: u64,
}

impl<C: Fn() -> Option<TaiNs>> VehicleReceiver<C> {
    /// Binds `config.link.listen`. `leap` is the signed leap-second table, `met` the signed
    /// mission epoch, `clock` the station TAI clock (`None` when it cannot tell the time).
    pub async fn bind(
        config: VehicleConfig,
        leap: LeapSeconds,
        met: MetEpoch,
        clock: C,
    ) -> io::Result<Self> {
        let socket = UdpSocket::bind(config.link.listen).await?;
        Ok(Self {
            socket,
            decoder: Decoder::new(config, leap, met),
            clock,
            buf: vec![0; MAX_DATAGRAM],
            no_station_time: 0,
        })
    }

    /// The bound address (the actual port when the configuration asked for port 0).
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// Waits for the next datagram and decodes it. Cancel-safe: dropping the future loses no
    /// decoded data (the datagram is only taken off the socket when the future completes).
    pub async fn recv(&mut self) -> Result<Decoded, RecvError> {
        let (len, from) = self.socket.recv_from(&mut self.buf).await?;
        let Some(received_at) = (self.clock)() else {
            self.no_station_time += 1;
            return Err(RecvError::NoStationTime);
        };
        let datagram = self.buf.get(..len).unwrap_or(&[]);
        Ok(self.decoder.decode(datagram, from.ip(), received_at))
    }

    /// Decoder counters; datagrams dropped for want of station time are in
    /// [`Self::no_station_time`].
    pub fn stats(&self) -> &Stats {
        self.decoder.stats()
    }

    /// Datagrams dropped because the station clock gave no time.
    pub fn no_station_time(&self) -> u64 {
        self.no_station_time
    }

    /// Vehicle time correlation at `now`.
    pub fn correlation(&self, now: TaiNs) -> CorrelationStatus {
        self.decoder.correlation(now)
    }

    /// The MID table in force.
    pub fn config(&self) -> &VehicleConfig {
        self.decoder.config()
    }
}
