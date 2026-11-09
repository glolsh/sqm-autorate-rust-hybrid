use crate::MeasurementType;
use icmp_socket::socket::IcmpSocket;
use icmp_socket::{IcmpSocket4, Icmpv4Packet};
use crate::util::RwLockExt;
use log::{debug, warn};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::mpsc::Sender;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use std::{io, thread};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum PingError {
    #[error("Couldn't parse number")]
    InvalidNumber(#[from] io::Error),
    #[error("Error parsing packet")]
    InvalidPacket(String),
    #[error("Invalid packet type")]
    InvalidType(String),
    #[error("Wrong ICMP identifier (expected {expected:?}, found {found:?})")]
    WrongID { expected: u16, found: u16 },
}

pub struct PingReply {
    pub reflector: IpAddr,
    pub seq: u16,
    pub rtt: i64,
    pub current_time: i64,
    pub down_time: f64,
    pub up_time: f64,
    pub originate_timestamp: i64,
    pub receive_timestamp: i64,
    pub transmit_timestamp: i64,
    pub last_receive_time_s: Instant,
    /// True if the sample came from an ICMP echo (RTT/2) rather than ICMP timestamps
    pub echo_based: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PingMode {
    Timestamp,
    Echo,
}

/// Consecutive unanswered requests before a reflector in timestamp mode falls back
/// to echo (~20 s at 0.5 s ticks). Also used to recognise the end of an outage.
const FALLBACK_AFTER_FAILS: u32 = 40;
/// How often a reflector in echo mode is re-tested for timestamp support
const TIMESTAMP_RETRY_SECS: u64 = 3600;
/// Number of timestamp probes sent after an outage ends
const RECOVERY_PROBES: u8 = 3;

/// Per-reflector measurement mode state machine (timestamps with echo fallback)
pub struct ReflectorState {
    pub current_mode: PingMode,
    pub consecutive_fails: u32,
    pub last_timestamp_attempt: Instant,
    /// Timestamp probes still to be sent while in echo mode
    pub timestamp_probes_left: u8,
}

impl Default for ReflectorState {
    fn default() -> Self {
        Self {
            current_mode: PingMode::Timestamp,
            consecutive_fails: 0,
            last_timestamp_attempt: Instant::now(),
            timestamp_probes_left: 0,
        }
    }
}

impl ReflectorState {
    /// Called for every request about to be sent; returns the request type to use.
    pub fn on_send(&mut self, seq: u16, now: Instant) -> PingMode {
        self.consecutive_fails = self.consecutive_fails.saturating_add(1);

        if self.current_mode == PingMode::Timestamp {
            if self.consecutive_fails < FALLBACK_AFTER_FAILS {
                return PingMode::Timestamp;
            }
            self.current_mode = PingMode::Echo;
            self.last_timestamp_attempt = now;
        }

        if now.duration_since(self.last_timestamp_attempt).as_secs() >= TIMESTAMP_RETRY_SECS {
            self.last_timestamp_attempt = now;
            return PingMode::Timestamp;
        }

        // Probe on every other request only, so echo samples keep flowing meanwhile
        if self.timestamp_probes_left > 0 && seq % 2 == 0 {
            self.timestamp_probes_left -= 1;
            return PingMode::Timestamp;
        }

        PingMode::Echo
    }

    pub fn on_echo_reply(&mut self) {
        // The first reply after a long silence means an outage just ended. The
        // fallback to echo may have been caused by that outage rather than by the
        // reflector filtering timestamps, so test timestamps again right away.
        if self.consecutive_fails >= FALLBACK_AFTER_FAILS {
            self.timestamp_probes_left = RECOVERY_PROBES;
        }
        self.consecutive_fails = 0;
    }

    pub fn on_timestamp_reply(&mut self) {
        self.consecutive_fails = 0;
        self.current_mode = PingMode::Timestamp;
        self.timestamp_probes_left = 0;
    }
}

fn open_socket(type_: MeasurementType) -> io::Result<IcmpSocket4> {
    match type_ {
        MeasurementType::Icmp | MeasurementType::IcmpTimestamps => {
            IcmpSocket4::new()
        },
        _ => {
            unimplemented!()
        }
    }
}

pub trait PingListener {
    fn listen(
        &mut self,
        id: u16,
        type_: MeasurementType,
        reflectors_lock: Arc<RwLock<Vec<IpAddr>>>,
        stats_sender: Sender<PingReply>,
    ) -> anyhow::Result<()> {
        let socket = &mut open_socket(type_)?;
        let mut consecutive_errors: u32 = 0;

        loop {
            let (pkt, sender) = match socket.rcv_from() {
                Ok(val) => val,
                Err(e) => {
                    // A single error is usually just an unrelated ICMP packet that failed
                    // to parse; sleeping then would delay (and thus distort) the next reply.
                    // Only back off when the socket is persistently failing, to avoid
                    // spinning at 100% CPU.
                    consecutive_errors += 1;
                    if consecutive_errors >= 100 {
                        debug!("ICMP receive keeps failing: {}", e);
                        thread::sleep(Duration::from_millis(100));
                    }
                    continue;
                }
            };
            consecutive_errors = 0;

            let addr: IpAddr = match sender.as_socket() {
                Some(sock) => sock.ip(),
                None => continue,
            };

            let reflectors = reflectors_lock.read_anyhow()?;
            if !reflectors.contains(&addr) {
                continue;
            }

            let reply = match self.parse_packet(id, addr, pkt) {
                Ok(val) => val,
                Err(_) => {
                    // parse_packet will throw an error if it's an unknown protocol etc.
                    // so just quietly move on
                    continue;
                }
            };

            debug!("Type: {:4}  | Reflector IP: {:>15}  | Seq: {:5}  | Current time: {:8}  |  Originate: {:8}  |  Received time: {:8}  |  Transmit time : {:8}  |  RTT: {:8}  | UL time: {:5}  | DL time: {:5}", "ICMP", addr.to_string(), reply.seq, reply.current_time, reply.originate_timestamp, reply.receive_timestamp, reply.transmit_timestamp, reply.rtt, reply.up_time, reply.down_time);
            if let Err(e) = stats_sender.send(reply) {
                warn!("Stats channel closed, stopping listener: {}", e);
                break Ok(());
            }
        }
    }

    fn parse_packet(&mut self, id: u16, reflector: IpAddr, pkt: Icmpv4Packet) -> Result<PingReply, PingError>;
}

pub trait PingSender {
    fn send(
        &mut self,
        id: u16,
        type_: MeasurementType,
        reflectors_lock: Arc<RwLock<Vec<IpAddr>>>,
        tick_interval: f64,
    ) -> anyhow::Result<()> {
        let mut socket = open_socket(type_)?;

        let mut seq: u16 = 0;
        let tick_duration_ms: u16 = (tick_interval * 1000.0) as u16;

        loop {
            // Clone the reflectors vec and drop the read lock immediately —
            // holding it for the entire tick would starve the reflector selector's write lock.
            let reflectors_unlocked = reflectors_lock.read_anyhow()?;
            let reflectors = reflectors_unlocked.clone();
            drop(reflectors_unlocked);

            if reflectors.is_empty() {
                thread::sleep(Duration::from_millis(tick_duration_ms as u64));
                continue;
            }

            let sleep_duration =
                Duration::from_millis((tick_duration_ms / reflectors.len() as u16) as u64);

            for reflector in reflectors.iter() {
                let addr: Ipv4Addr = match reflector {
                    IpAddr::V4(ipv4) => *ipv4,
                    IpAddr::V6(_) => continue, // IPv6 reflectors are not supported
                };

                let packet = self.craft_packet(id, seq, *reflector);
                // A failed send (e.g. network unreachable while the WAN is down) is
                // transient and must not take the whole daemon down.
                if let Err(e) = socket.send_to(addr, packet) {
                    debug!("Failed to send ping to {}: {}", addr, e);
                }
                thread::sleep(sleep_duration);
            }

            if seq == u16::MAX {
                seq = 0;
            } else {
                seq += 1;
            }
        }
    }

    fn craft_packet(&mut self, id: u16, seq: u16, reflector: IpAddr) -> icmp_socket::packet::Icmpv4Packet;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Send `n` requests without any reply; returns the modes used
    fn send_n(st: &mut ReflectorState, n: u32, now: Instant) -> Vec<PingMode> {
        (0..n).map(|i| st.on_send(i as u16, now)).collect()
    }

    #[test]
    fn stays_in_timestamp_mode_while_replies_arrive() {
        let mut st = ReflectorState::default();
        let now = Instant::now();
        for seq in 0..200 {
            assert_eq!(st.on_send(seq, now), PingMode::Timestamp);
            st.on_timestamp_reply();
        }
    }

    #[test]
    fn falls_back_to_echo_after_silence() {
        let mut st = ReflectorState::default();
        let now = Instant::now();
        let modes = send_n(&mut st, FALLBACK_AFTER_FAILS, now);
        assert!(modes[..modes.len() - 1].iter().all(|m| *m == PingMode::Timestamp));
        assert_eq!(*modes.last().unwrap(), PingMode::Echo);
        assert_eq!(st.current_mode, PingMode::Echo);
    }

    #[test]
    fn probes_timestamps_after_outage_and_recovers() {
        let mut st = ReflectorState::default();
        let now = Instant::now();
        send_n(&mut st, FALLBACK_AFTER_FAILS + 10, now);

        // link is back: first echo reply schedules probes
        st.on_echo_reply();
        assert_eq!(st.timestamp_probes_left, RECOVERY_PROBES);

        // probes go out on even sequence numbers only
        assert_eq!(st.on_send(1, now), PingMode::Echo);
        assert_eq!(st.on_send(2, now), PingMode::Timestamp);

        st.on_timestamp_reply();
        assert_eq!(st.current_mode, PingMode::Timestamp);
        assert_eq!(st.on_send(3, now), PingMode::Timestamp);
    }

    #[test]
    fn filtering_reflector_settles_in_echo_mode() {
        let mut st = ReflectorState::default();
        let now = Instant::now();
        send_n(&mut st, FALLBACK_AFTER_FAILS, now);

        // echo works, timestamps never do
        let mut probes = 0;
        for seq in 0..100u16 {
            st.on_echo_reply();
            if st.on_send(seq, now) == PingMode::Timestamp {
                probes += 1;
            }
        }
        assert_eq!(probes, RECOVERY_PROBES as u32);
        assert_eq!(st.current_mode, PingMode::Echo);
    }

    #[test]
    fn retries_timestamps_periodically() {
        let mut st = ReflectorState::default();
        let now = Instant::now();
        send_n(&mut st, FALLBACK_AFTER_FAILS, now);
        st.on_echo_reply();
        st.timestamp_probes_left = 0;

        assert_eq!(st.on_send(1, now), PingMode::Echo);
        let later = now + Duration::from_secs(TIMESTAMP_RETRY_SECS);
        assert_eq!(st.on_send(3, later), PingMode::Timestamp);
        assert_eq!(st.on_send(5, later), PingMode::Echo);
    }
}
