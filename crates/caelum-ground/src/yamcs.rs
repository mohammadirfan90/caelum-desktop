//! Yamcs link (F-GND-03): CAELUM telemetry packets into a Yamcs TM data link.
//!
//! - UDP: one packet per datagram to Yamcs' `UdpTmDataLink` port.
//! - TCP: Yamcs' `TcpTmDataLink` is a TCP *client*; CAELUM listens and streams packets back to
//!   back to every connected Yamcs instance. A failed connection is dropped and Yamcs reconnects.
//!
//! [`instance_config`] renders the Yamcs instance configuration fragment (data link plus the XTCE
//! MDB from [`crate::xtce`]) used by the CI job that decodes CAELUM packets in a Yamcs container.

use std::io::{self, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};

use crate::publisher::PacketSink;

/// Transport to Yamcs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YamcsTransport {
    /// Datagrams to the Yamcs UDP port.
    Udp(SocketAddr),
    /// Listen on this address for Yamcs TCP clients.
    TcpListen(SocketAddr),
}

/// Link statistics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LinkStats {
    /// Packets written (counted once per packet, not per client).
    pub packets: u64,
    /// Send failures.
    pub failures: u64,
    /// Connected TCP clients.
    pub clients: usize,
}

/// A Yamcs telemetry link.
#[derive(Debug)]
pub struct YamcsLink {
    udp: Option<(UdpSocket, SocketAddr)>,
    listener: Option<TcpListener>,
    clients: Vec<TcpStream>,
    stats: LinkStats,
}

impl YamcsLink {
    /// Opens the link.
    pub fn open(transport: YamcsTransport) -> io::Result<Self> {
        let mut link = Self {
            udp: None,
            listener: None,
            clients: Vec::new(),
            stats: LinkStats::default(),
        };
        match transport {
            YamcsTransport::Udp(to) => {
                let bind: SocketAddr = if to.is_ipv4() {
                    SocketAddr::from(([0, 0, 0, 0], 0))
                } else {
                    SocketAddr::from(([0u16; 8], 0))
                };
                link.udp = Some((UdpSocket::bind(bind)?, to));
            }
            YamcsTransport::TcpListen(at) => {
                let l = TcpListener::bind(at)?;
                l.set_nonblocking(true)?;
                link.listener = Some(l);
            }
        }
        Ok(link)
    }

    /// The TCP listening address.
    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.listener.as_ref().and_then(|l| l.local_addr().ok())
    }

    /// Accepts pending Yamcs connections.
    pub fn accept(&mut self) -> io::Result<()> {
        let Some(l) = &self.listener else {
            return Ok(());
        };
        loop {
            match l.accept() {
                Ok((s, _)) => {
                    s.set_nonblocking(false)?;
                    s.set_nodelay(true)?;
                    self.clients.push(s);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            }
        }
        self.stats.clients = self.clients.len();
        Ok(())
    }

    /// Statistics.
    pub fn stats(&self) -> LinkStats {
        self.stats
    }
}

impl PacketSink for YamcsLink {
    fn send(&mut self, packet: &[u8]) -> io::Result<()> {
        if let Some((socket, to)) = &self.udp {
            return match socket.send_to(packet, *to) {
                Ok(n) if n == packet.len() => {
                    self.stats.packets += 1;
                    Ok(())
                }
                Ok(_) => {
                    self.stats.failures += 1;
                    Err(io::Error::new(io::ErrorKind::WriteZero, "short datagram"))
                }
                Err(e) => {
                    self.stats.failures += 1;
                    Err(e)
                }
            };
        }
        self.accept()?;
        let before = self.clients.len();
        self.clients.retain_mut(|c| c.write_all(packet).is_ok());
        self.stats.failures += (before - self.clients.len()) as u64;
        self.stats.clients = self.clients.len();
        if self.clients.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "no Yamcs client connected",
            ));
        }
        self.stats.packets += 1;
        Ok(())
    }
}

/// The Yamcs instance configuration fragment (YAML) for a CAELUM data link and XTCE MDB.
pub fn instance_config(transport: YamcsTransport, mdb_file: &str) -> String {
    let link = match transport {
        YamcsTransport::Udp(to) => format!(
            "  - name: caelum-tm\n    class: org.yamcs.tctm.UdpTmDataLink\n    stream: tm_realtime\n    port: {}\n",
            to.port()
        ),
        YamcsTransport::TcpListen(at) => format!(
            "  - name: caelum-tm\n    class: org.yamcs.tctm.TcpTmDataLink\n    stream: tm_realtime\n    host: {}\n    port: {}\n",
            at.ip(),
            at.port()
        ),
    };
    format!(
        "# CAELUM telemetry (F-GND-03). Packet time is the CUC secondary header (1958 TAI epoch).\n\
         dataLinks:\n{link}    packetPreprocessorClassName: org.yamcs.tctm.GenericPacketPreprocessor\n    packetPreprocessorArgs:\n      timestampOffset: -1\n      seqCountOffset: 2\n      errorDetection:\n        type: NONE\n\
         mdb:\n  - type: xtce\n    args:\n      file: {mdb_file}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::tests::{alarm, plan};
    use crate::ops::OpsRecord;
    use crate::packet::ParsedTm;
    use std::io::Read;
    use std::time::Duration;

    fn packet() -> Vec<u8> {
        OpsRecord::Alarm(alarm(2_000_000_000_000_000_000))
            .to_packet(&plan(), 1)
            .expect("packet")
            .into_bytes()
    }

    /// F-GND-03: UDP link sends one packet per datagram.
    #[test]
    fn f_gnd_03_udp_link_sends_packets() {
        let rx = UdpSocket::bind("127.0.0.1:0").expect("bind");
        rx.set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout");
        let mut link =
            YamcsLink::open(YamcsTransport::Udp(rx.local_addr().expect("addr"))).expect("open");
        link.send(&packet()).expect("sent");
        let mut buf = [0u8; 128];
        let n = rx.recv(&mut buf).expect("datagram");
        assert!(ParsedTm::parse(&buf[..n]).is_ok());
        assert_eq!(link.stats().packets, 1);
    }

    /// F-GND-03: TCP link streams back-to-back packets to a connected Yamcs client; with no
    /// client it reports the failure (the caller keeps the packets).
    #[test]
    fn f_gnd_03_tcp_link_streams_to_yamcs_clients() {
        let mut link = YamcsLink::open(YamcsTransport::TcpListen(
            "127.0.0.1:0".parse().expect("addr"),
        ))
        .expect("open");
        assert!(link.send(&packet()).is_err(), "no client yet");
        let addr = link.local_addr().expect("listening");
        let mut client = TcpStream::connect(addr).expect("connect");
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout");
        let mut sent = false;
        for _ in 0..50 {
            if link.send(&packet()).is_ok() {
                sent = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(sent, "client accepted");
        link.send(&packet()).expect("second");
        let one = packet().len();
        let mut buf = vec![0u8; 2 * one];
        client.read_exact(&mut buf).expect("two packets");
        let (_, rest) = ParsedTm::parse(&buf).expect("first");
        let (_, rest) = ParsedTm::parse(rest).expect("second");
        assert!(rest.is_empty());
        assert_eq!(link.stats().clients, 1);
    }

    #[test]
    fn instance_config_names_link_and_mdb() {
        let c = instance_config(
            YamcsTransport::Udp("10.0.0.5:10015".parse().expect("addr")),
            "mdb/caelum-ops.xml",
        );
        assert!(c.contains("UdpTmDataLink"));
        assert!(c.contains("port: 10015"));
        assert!(c.contains("file: mdb/caelum-ops.xml"));
        let t = instance_config(
            YamcsTransport::TcpListen("0.0.0.0:10016".parse().expect("addr")),
            "m.xml",
        );
        assert!(t.contains("TcpTmDataLink") && t.contains("port: 10016"));
    }
}
