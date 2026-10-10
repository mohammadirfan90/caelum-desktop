//! Path B status publisher (F-GND-11 publish side, F-GND-13, SR-011, SR-015, R-SEC-06).
//!
//! Sends OPS records as telemetry-only CCSDS packets directly to the vehicle: to a configurable
//! UDP endpoint until ICD-VEH OI-1 is agreed. Every record first goes into a dedicated OPS-only
//! store-and-forward queue (persisted, signed), so a record survives an unavailable endpoint or
//! a restart and is sent when the endpoint accepts it (ICD-VEH §9).
//!
//! The publisher depends on nothing but its queue and its socket: the onboard medical data
//! system is not in this path (SR-015). It can only take [`OpsRecord`]s and can only build
//! [`crate::packet::TmPacket`]s, so it can neither carry physiology nor send a telecommand.

use std::io;
use std::net::{SocketAddr, UdpSocket};

use caelum_alarms::escalation::OpsEscalation;
use caelum_domain::parameters::Channel;
use caelum_domain::time::TaiNs;

use crate::ops::{OpsAlarmRecord, OpsError, OpsRecord};
use crate::queue::{
    DownlinkQueue, FileItem, FileSource, ItemSigner, Payload, QueueError, QueuePriority, QueueStore,
};

/// Publisher errors.
#[derive(Debug, thiserror::Error)]
pub enum PublishError {
    /// The record could not be converted.
    #[error(transparent)]
    Ops(#[from] OpsError),
    /// The record could not be queued (not persisted).
    #[error(transparent)]
    Queue(#[from] QueueError),
    /// Socket setup failed.
    #[error("socket: {0}")]
    Io(#[from] io::Error),
}

/// Where packets go.
pub trait PacketSink {
    /// Sends one packet.
    fn send(&mut self, packet: &[u8]) -> io::Result<()>;
}

/// A UDP sink to a fixed endpoint (path B, ICD-VEH OI-1 interim).
#[derive(Debug)]
pub struct UdpSink {
    socket: UdpSocket,
    endpoint: SocketAddr,
}

impl UdpSink {
    /// Binds an ephemeral local socket for sending to `endpoint`.
    pub fn new(endpoint: SocketAddr) -> io::Result<Self> {
        let bind: SocketAddr = if endpoint.is_ipv4() {
            SocketAddr::from(([0, 0, 0, 0], 0))
        } else {
            SocketAddr::from(([0u16; 8], 0))
        };
        Ok(Self {
            socket: UdpSocket::bind(bind)?,
            endpoint,
        })
    }
}

impl PacketSink for UdpSink {
    fn send(&mut self, packet: &[u8]) -> io::Result<()> {
        let n = self.socket.send_to(packet, self.endpoint)?;
        if n == packet.len() {
            Ok(())
        } else {
            Err(io::Error::new(io::ErrorKind::WriteZero, "short datagram"))
        }
    }
}

/// The OPS-only queue never holds files.
struct NoFiles;

impl FileSource for NoFiles {
    fn read(&self, item: &FileItem) -> Result<Vec<u8>, QueueError> {
        Err(QueueError::FileMismatch(item.name.clone()))
    }
}

/// Outcome of a flush.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FlushReport {
    /// Packets accepted by the sink.
    pub sent: usize,
    /// Records still waiting (endpoint unavailable).
    pub waiting: usize,
}

/// The path B publisher.
pub struct PathBPublisher<S: QueueStore, K: PacketSink> {
    queue: DownlinkQueue<S>,
    sink: K,
}

impl<S: QueueStore, K: PacketSink> PathBPublisher<S, K> {
    /// A publisher over its own queue (opened on a path B store) and a sink. Path B is not bound
    /// to ground contact windows: the vehicle endpoint is onboard.
    pub fn new(mut queue: DownlinkQueue<S>, sink: K) -> Self {
        queue.set_link_override(Some(true));
        Self { queue, sink }
    }

    /// Publishes a record: queued (persisted, signed) first, then sent with the backlog.
    pub fn publish(
        &mut self,
        record: OpsRecord,
        now: TaiNs,
        signer: &dyn ItemSigner,
    ) -> Result<FlushReport, PublishError> {
        self.queue.enqueue(
            Channel::Ops,
            QueuePriority::Critical,
            Payload::Ops(record),
            now,
            signer,
        )?;
        self.flush(now)
    }

    /// Publishes the ground stage of an escalation (docs/06 §3a, F-ALM-04).
    pub fn publish_escalation(
        &mut self,
        e: &OpsEscalation,
        signer: &dyn ItemSigner,
    ) -> Result<FlushReport, PublishError> {
        let r = OpsAlarmRecord::try_from(e)?;
        self.publish(OpsRecord::Alarm(r), r.at, signer)
    }

    /// Sends the backlog in order; stops at the first sink failure (retried at the next flush).
    pub fn flush(&mut self, now: TaiNs) -> Result<FlushReport, PublishError> {
        let pending = self.queue.stats().pending;
        let out = self.queue.tick(now, pending, &NoFiles)?;
        let mut sent = 0;
        for o in out {
            if self.sink.send(&o.packet).is_err() {
                break;
            }
            if let Some(id) = o.item {
                self.queue.delivered(id)?;
            }
            sent += 1;
        }
        Ok(FlushReport {
            sent,
            waiting: self.queue.stats().pending,
        })
    }

    /// The sink (tests, link statistics).
    pub fn sink(&self) -> &K {
        &self.sink
    }

    /// Releases the queue (restart).
    pub fn into_queue(self) -> DownlinkQueue<S> {
        self.queue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfdp::tests::cfg;
    use crate::config::ContactSchedule;
    use crate::ops::tests::{alarm, crew, plan};
    use crate::packet::ParsedTm;
    use crate::queue::{AllowLists, MemoryStore};
    use ed25519_dalek::SigningKey;
    use std::time::Duration;

    const T: i64 = 2_100_000_000_000_000_000;

    struct Flaky {
        up: bool,
        sent: Vec<Vec<u8>>,
    }

    impl PacketSink for Flaky {
        fn send(&mut self, packet: &[u8]) -> io::Result<()> {
            if self.up {
                self.sent.push(packet.to_vec());
                Ok(())
            } else {
                Err(io::Error::new(io::ErrorKind::ConnectionRefused, "down"))
            }
        }
    }

    fn queue(store: MemoryStore) -> DownlinkQueue<MemoryStore> {
        DownlinkQueue::open(
            store,
            plan(),
            AllowLists::from_catalog(),
            ContactSchedule::default(),
            cfg(10, 1),
            SigningKey::from_bytes(&[42; 32]).verifying_key(),
            &NoFiles,
        )
        .expect("opens")
    }

    /// F-GND-11, SR-011: a published escalation arrives as a telemetry packet on the OPS alarm
    /// APID over UDP, decodable with the OPS fields only.
    #[test]
    fn f_gnd_11_escalation_reaches_the_udp_endpoint_as_tm() {
        let rx = UdpSocket::bind("127.0.0.1:0").expect("bind");
        rx.set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout");
        let sink = UdpSink::new(rx.local_addr().expect("addr")).expect("sink");
        let mut p = PathBPublisher::new(queue(MemoryStore::default()), sink);
        let k = SigningKey::from_bytes(&[42; 32]);
        let e = OpsEscalation {
            crew_id: crew(),
            priority: "critical",
            category: "CARDIO",
            state: "active",
            stage: 4,
            created_at_tai: T,
            at_tai: T + 60_000_000_000,
        };
        let report = p.publish_escalation(&e, &k).expect("published");
        assert_eq!(
            report,
            FlushReport {
                sent: 1,
                waiting: 0
            }
        );
        let mut buf = [0u8; 256];
        let n = rx.recv(&mut buf).expect("datagram");
        assert_eq!(buf[0] & 0x10, 0, "telemetry, never a telecommand");
        let (tm, _) = ParsedTm::parse(&buf[..n]).expect("TM");
        assert_eq!(tm.apid, plan().ops_alarm);
        let r = OpsRecord::decode(&plan(), tm.apid, tm.time, &tm.user_data).expect("OPS");
        assert_eq!(
            r,
            OpsRecord::Alarm(OpsAlarmRecord::try_from(&e).expect("converts"))
        );
    }

    /// SR-015, ICD-VEH §9: while the endpoint is unavailable records wait (persisted) and are
    /// sent once, in order, when it accepts again — also across a restart.
    #[test]
    fn sr_015_records_wait_for_the_endpoint_and_survive_restart() {
        let k = SigningKey::from_bytes(&[42; 32]);
        let mut p = PathBPublisher::new(
            queue(MemoryStore::default()),
            Flaky {
                up: false,
                sent: Vec::new(),
            },
        );
        for i in 0..3 {
            let r = p
                .publish(
                    OpsRecord::Alarm(alarm(T + i * 1_000_000_000)),
                    TaiNs(T + i),
                    &k,
                )
                .expect("queued");
            assert_eq!(r.sent, 0);
        }
        let store = p.into_queue().into_store();
        let mut p = PathBPublisher::new(
            queue(store),
            Flaky {
                up: true,
                sent: Vec::new(),
            },
        );
        let r = p.flush(TaiNs(T + 10)).expect("flush");
        assert_eq!(
            r,
            FlushReport {
                sent: 3,
                waiting: 0
            }
        );
        let decoded: Vec<i64> = p
            .sink()
            .sent
            .iter()
            .map(|b| {
                let (tm, _) = ParsedTm::parse(b).expect("TM");
                match OpsRecord::decode(&plan(), tm.apid, tm.time, &tm.user_data).expect("OPS") {
                    OpsRecord::Alarm(a) => a.at.0,
                    other => panic!("unexpected {other:?}"),
                }
            })
            .collect();
        assert_eq!(decoded.len(), 3);
        assert!(decoded.windows(2).all(|w| w[0] < w[1]), "in order");
        assert_eq!(p.flush(TaiNs(T + 11)).expect("flush").sent, 0, "sent once");
    }

    #[test]
    fn unknown_escalation_names_are_refused() {
        let k = SigningKey::from_bytes(&[42; 32]);
        let mut p = PathBPublisher::new(
            queue(MemoryStore::default()),
            Flaky {
                up: true,
                sent: Vec::new(),
            },
        );
        let e = OpsEscalation {
            crew_id: crew(),
            priority: "urgent",
            category: "CARDIO",
            state: "active",
            stage: 4,
            created_at_tai: T,
            at_tai: T,
        };
        assert!(matches!(
            p.publish_escalation(&e, &k),
            Err(PublishError::Ops(_))
        ));
        assert!(p.sink().sent.is_empty());
    }
}
