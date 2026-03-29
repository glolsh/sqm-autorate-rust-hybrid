use crate::pinger::{PingError, PingListener, PingMode, PingReply, PingSender, ReflectorState};
use crate::time::Time;
use icmp_socket::packet::{WithEchoRequest, WithTimestampRequest};
use icmp_socket::Icmpv4Message;
use icmp_socket::Icmpv4Packet;
use rustix::thread::ClockId;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, RwLock};
use std::time::Instant;

pub struct PingerICMPTimestampListener {
    pub state: Arc<RwLock<HashMap<IpAddr, ReflectorState>>>,
}

pub struct PingerICMPTimestampSender {
    pub state: Arc<RwLock<HashMap<IpAddr, ReflectorState>>>,
}

impl PingListener for PingerICMPTimestampListener {
    // Result: RTT, down time, up time
    fn parse_packet(&mut self, id: u16, reflector: IpAddr, pkt: Icmpv4Packet) -> Result<PingReply, PingError> {
        match pkt.typ {
            // 0 = Echo Reply (fallback)
            0 => {
                if let Icmpv4Message::EchoReply {
                    identifier,
                    sequence,
                    payload,
                } = pkt.message
                {
                    if identifier != id {
                        return Err(PingError::WrongID {
                            expected: id,
                            found: identifier,
                        });
                    }

                    if let Ok(mut state_map) = self.state.write() {
                        let st = state_map.entry(reflector).or_insert_with(ReflectorState::default);
                        st.consecutive_fails = 0;
                        if st.current_mode == PingMode::Echo && st.last_timestamp_attempt.elapsed().as_secs() < 3600 {
                            // keep in echo mode, unless we successfully tested a timestamp reply (handled below in type 14)
                        } else {
                            // We shouldn't really get echo replies if we sent timestamp requests, but reset fails anyway.
                        }
                    }

                    let time_sent = match payload.as_slice().try_into() {
                        Ok(bytes) => u64::from_be_bytes(bytes) as i64,
                        Err(_) => {
                            return Err(PingError::InvalidPacket(format!("Expected 8 bytes payload, but found {}", payload.len())))
                        }
                    };

                    let clock = Time::new(ClockId::Monotonic);
                    let time_ms = clock.to_milliseconds() as i64;

                    let rtt: i64 = time_ms - time_sent;
                    Ok(PingReply {
                        reflector,
                        seq: sequence,
                        rtt,
                        current_time: time_ms,
                        down_time: (rtt as f64) / 2.0,
                        up_time: (rtt as f64) / 2.0,
                        originate_timestamp: time_sent,
                        receive_timestamp: 0,
                        transmit_timestamp: 0,
                        last_receive_time_s: Instant::now(),
                    })
                } else {
                    Err(PingError::InvalidPacket(format!("Packet had type {:?}, but did not match the structure", pkt.typ)))
                }
            },
            // 14 = Timestamp reply
            14 => {
                if let Icmpv4Message::TimestampReply {
                    identifier,
                    sequence,
                    originate,
                    receive,
                    transmit,
                } = pkt.message
                {
                    if identifier != id {
                        return Err(PingError::WrongID {
                            expected: id,
                            found: identifier,
                        });
                    }

                    if let Ok(mut state_map) = self.state.write() {
                        let st = state_map.entry(reflector).or_insert_with(ReflectorState::default);
                        st.consecutive_fails = 0;
                        st.current_mode = PingMode::Timestamp;
                    }

                    let time_now = Time::new(ClockId::Realtime);
                    let time_since_midnight = time_now.get_time_since_midnight();

                    let rtt: i64 = time_since_midnight - originate as i64;
                    let dl_time: i64 = time_since_midnight - transmit as i64;
                    let ul_time: i64 = receive as i64 - originate as i64;

                    Ok(PingReply {
                        reflector,
                        seq: sequence,
                        rtt,
                        current_time: time_since_midnight,
                        down_time: dl_time as f64,
                        up_time: ul_time as f64,
                        originate_timestamp: originate as i64,
                        receive_timestamp: receive as i64,
                        transmit_timestamp: transmit as i64,
                        last_receive_time_s: Instant::now(),
                    })
                } else {
                    Err(PingError::InvalidPacket(format!("Packet had type {:?}, but did not match the structure", pkt.typ)))
                }

                
            },
            type_ => Err(PingError::InvalidType(format!("{:?}", type_))),
        }
    }
}

impl PingSender for PingerICMPTimestampSender {
    fn craft_packet(&mut self, id: u16, seq: u16, reflector: IpAddr) -> Icmpv4Packet {
        let mut mode = PingMode::Timestamp;

        if let Ok(mut state_map) = self.state.write() {
            let st = state_map.entry(reflector).or_insert_with(ReflectorState::default);

            st.consecutive_fails += 1;

            if st.current_mode == PingMode::Timestamp && st.consecutive_fails >= 10 {
                st.current_mode = PingMode::Echo;
                st.last_timestamp_attempt = Instant::now();
            }

            if st.current_mode == PingMode::Echo && st.last_timestamp_attempt.elapsed().as_secs() >= 3600 {
                mode = PingMode::Timestamp;
                st.last_timestamp_attempt = Instant::now();
            } else {
                mode = st.current_mode;
            }
        }

        match mode {
            PingMode::Timestamp => {
                let time_since_midnight = Time::new(ClockId::Realtime).get_time_since_midnight();
                Icmpv4Packet::with_timestamp_request(id, seq, time_since_midnight as u32, 0, 0).unwrap()
            }
            PingMode::Echo => {
                let clock = Time::new(ClockId::Monotonic);
                let time_ms = clock.to_milliseconds();
                let payload = time_ms.to_be_bytes().to_vec();
                Icmpv4Packet::with_echo_request(id, seq, payload).unwrap()
            }
        }
    }
}
