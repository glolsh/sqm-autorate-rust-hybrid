use crate::netlink::{Netlink, NetlinkError, Qdisc};
use crate::time::Time;
use crate::util::{MutexExt, RwLockExt};
use crate::{Config, ReflectorStats};
use log::{debug, info, warn};
use rustix::thread::ClockId;
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::net::IpAddr;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, RwLock};
use std::thread::sleep;
use std::time::{Duration, Instant};
use thiserror::Error;

#[derive(Copy, Clone, Debug, PartialEq)]
enum Direction {
    Down,
    Up,
}

#[derive(Debug, Error)]
pub enum RatecontrolError {
    #[error("Netlink error")]
    Netlink(#[from] NetlinkError),
}

#[derive(Copy, Clone, Debug)]
pub enum StatsDirection {
    RX,
    TX,
}

fn get_interface_stats(
    config: &Config,
    down_direction: StatsDirection,
    up_direction: StatsDirection,
) -> Result<(i128, i128), RatecontrolError> {
    let (down_rx, down_tx) = Netlink::get_interface_stats(config.download_interface.as_str())?;
    let (up_rx, up_tx) = Netlink::get_interface_stats(config.upload_interface.as_str())?;

    let rx_bytes = match down_direction {
        StatsDirection::RX => down_rx,
        StatsDirection::TX => down_tx,
    };

    let tx_bytes = match up_direction {
        StatsDirection::RX => up_rx,
        StatsDirection::TX => up_tx,
    };

    Ok((rx_bytes.into(), tx_bytes.into()))
}

#[derive(Clone, Debug)]
struct State {
    current_bytes: i128,
    current_rate: f64,
    delta_stat: f64,
    deltas: Vec<f64>,
    qdisc: Qdisc,
    load: f64,
    next_rate: f64,
    previous_bytes: i128,
    prev_t: Instant,
    utilisation: f64,
    cooldown_until: Option<Instant>,
    congestion_ceiling: f64,
    status: String,
}

impl State {
    fn new(qdisc: Qdisc, previous_bytes: i128) -> Self {
        State {
            current_bytes: 0,
            current_rate: 0.0,
            delta_stat: 0.0,
            deltas: Vec::new(),
            load: 0.0,
            next_rate: 0.0,
            qdisc,
            previous_bytes,
            prev_t: Instant::now(),
            utilisation: 0.0,
            cooldown_until: None,
            congestion_ceiling: 0.0,
            status: "INIT".to_string(),
        }
    }
}

pub struct Ratecontroller {
    config: Config,
    down_direction: StatsDirection,
    owd_baseline: Arc<Mutex<HashMap<IpAddr, ReflectorStats>>>,
    owd_recent: Arc<Mutex<HashMap<IpAddr, ReflectorStats>>>,
    reflectors_lock: Arc<RwLock<Vec<IpAddr>>>,
    reselect_trigger: Sender<bool>,
    state_dl: State,
    state_ul: State,
    up_direction: StatsDirection,
}

impl Ratecontroller {
    fn calculate_rate(&mut self, direction: Direction) -> anyhow::Result<()> {
        let (delay_ms, min_rate, max_rate, state) = if direction == Direction::Down {
            (
                self.config.download_delay_ms,
                self.config.download_min_kbits,
                self.config.download_max_kbits,
                &mut self.state_dl,
            )
        } else {
            (
                self.config.upload_delay_ms,
                self.config.upload_min_kbits,
                self.config.upload_max_kbits,
                &mut self.state_ul,
            )
        };

        let now_t = Instant::now();
        let dur = now_t.duration_since(state.prev_t);

        if !state.deltas.is_empty() {
            state.next_rate = state.current_rate;

            state.delta_stat = if state.deltas.len() >= 3 {
                state.deltas[2]
            } else {
                state.deltas[0]
            };

            debug!(
                "Sorted {:?} deltas: {:?} | Selected: {}",
                direction, state.deltas, state.delta_stat
            );

            if state.delta_stat > 0.0 {
                /*
                 * TODO - find where the (8 / 1000) comes from and
                 *    i. convert to a pre-computed factor
                 *    ii. ideally, see if it can be defined in terms of constants, eg ticks per second and number of active reflectors
                 */
                state.utilisation = (8.0 / 1000.0)
                    * (state.current_bytes as f64 - state.previous_bytes as f64)
                    / dur.as_secs_f64();
                state.load = state.utilisation / state.current_rate;

                debug!(
                    "direction: {:?} | util: {:.2} Kbps | load: {:.4}",
                    direction, state.utilisation, state.load
                );

                let in_cooldown = match state.cooldown_until {
                    Some(time) => now_t < time,
                    None => false,
                };

                if in_cooldown {
                    state.status = "COOLDOWN".to_string();
                    state.next_rate = state.current_rate;
                } else if state.delta_stat > delay_ms {
                    state.status = "CONGESTION".to_string();

                    // Protect against idle lag spikes: never calculate the drop from a base lower than 50% of the current limit
                    let effective_baseline = state.utilisation.max(state.current_rate * 0.5);
                    state.next_rate = effective_baseline * self.config.decrease_multiplier;

                    state.congestion_ceiling = state.utilisation;
                    state.cooldown_until = Some(now_t + Duration::from_secs_f64(self.config.cooldown_secs));
                } else if state.delta_stat <= delay_ms && state.load > self.config.high_load_level {
                    state.status = "PROBING".to_string();

                    // Additive increase
                    state.next_rate = state.current_rate + self.config.increase_step_kbits;

                    // Optional guard: if approaching ceiling, could reduce step size,
                    // but standard AIMD continues adding until next drop.
                } else {
                    state.status = "HOLD".to_string();
                    state.next_rate = state.current_rate;
                }
            }
        }

        if max_rate > 0.0 {
            state.next_rate = state.next_rate.min(max_rate);
        }

        state.next_rate = state.next_rate.max(min_rate).floor();
        state.previous_bytes = state.current_bytes;
        state.prev_t = now_t;

        Ok(())
    }

    fn update_deltas(&mut self) -> anyhow::Result<()> {
        let state_dl = &mut self.state_dl;
        let state_ul = &mut self.state_ul;

        state_dl.deltas.clear();
        state_ul.deltas.clear();

        let now_t = Instant::now();
        let owd_baseline = self.owd_baseline.lock_anyhow()?;
        let owd_recent = self.owd_recent.lock_anyhow()?;
        let reflectors = self.reflectors_lock.read_anyhow()?;

        for reflector in reflectors.iter() {
            // only consider this data if it's less than 2 * tick_duration seconds old
            if owd_baseline.contains_key(reflector)
                && owd_recent.contains_key(reflector)
                && now_t
                    .duration_since(owd_recent[reflector].last_receive_time_s)
                    .as_secs_f64()
                    < self.config.tick_interval * 2.0
            {
                state_dl
                    .deltas
                    .push(owd_recent[reflector].down_ewma - owd_baseline[reflector].down_ewma);
                state_ul
                    .deltas
                    .push(owd_recent[reflector].up_ewma - owd_baseline[reflector].up_ewma);

                debug!(
                    "Reflector: {} down_delay: {} up_delay: {}",
                    reflector,
                    state_dl.deltas.last().unwrap(),
                    state_ul.deltas.last().unwrap()
                );
            }
        }

        // sort owd's lowest to highest
        state_dl.deltas.sort_by(|a, b| a.total_cmp(b));
        state_ul.deltas.sort_by(|a, b| a.total_cmp(b));

        let required_deltas = std::cmp::min(3, reflectors.len());
        if state_dl.deltas.len() < required_deltas || state_ul.deltas.len() < required_deltas {
            // trigger reselection
            warn!("Not enough delta values (required: {}), triggering reselection", required_deltas);
            let _ = self.reselect_trigger.send(true);
        }

        Ok(())
    }

    pub fn new(
        config: Config,
        owd_baseline: Arc<Mutex<HashMap<IpAddr, ReflectorStats>>>,
        owd_recent: Arc<Mutex<HashMap<IpAddr, ReflectorStats>>>,
        reflectors_lock: Arc<RwLock<Vec<IpAddr>>>,
        reselect_trigger: Sender<bool>,
        down_direction: StatsDirection,
        up_direction: StatsDirection,
    ) -> anyhow::Result<Self> {
        let dl_qdisc = Netlink::qdisc_from_ifname(config.download_interface.as_str())?;
        let ul_qdisc = Netlink::qdisc_from_ifname(config.upload_interface.as_str())?;

        let (cur_rx, cur_tx) = get_interface_stats(&config, down_direction, up_direction)?;

        Ok(Self {
            config,
            down_direction,
            owd_baseline,
            owd_recent,
            reflectors_lock,
            reselect_trigger,
            state_dl: State::new(dl_qdisc, cur_rx),
            state_ul: State::new(ul_qdisc, cur_tx),
            up_direction,
        })
    }

    pub fn run(&mut self) -> anyhow::Result<()> {
        let sleep_time = Duration::from_secs_f64(self.config.min_change_interval);

        let mut lastchg_t = Instant::now();

        // set qdisc rates to 95% of base rate to avoid load traps
        self.state_dl.current_rate = self.config.download_base_kbits * 0.95;
        self.state_ul.current_rate = self.config.upload_base_kbits * 0.95;

        Netlink::set_qdisc_rate(
            self.state_dl.qdisc,
            self.state_dl.current_rate.round() as u64,
            self.config.cake_ack_filter,
            &self.config.cake_rtt,
        )?;
        Netlink::set_qdisc_rate(
            self.state_ul.qdisc,
            self.state_ul.current_rate.round() as u64,
            self.config.cake_ack_filter,
            &self.config.cake_rtt,
        )?;

        let mut stats_fd: Option<File> = None;
        let mut stats_fd_inner: File;

        if !self.config.suppress_statistics {
            stats_fd_inner = File::options()
                .create(true)
                .write(true)
                .truncate(true)
                .open(self.config.stats_file.as_str())?;

            stats_fd_inner.write_all(
                "times,timens,rxload,txload,deltadelaydown,deltadelayup,dlrate,uprate\n".as_bytes(),
            )?;
            stats_fd_inner.flush()?;

            stats_fd = Some(stats_fd_inner);
        }

        loop {
            sleep(sleep_time);
            let now_t = Instant::now();

            if now_t.duration_since(lastchg_t).as_secs_f64() > self.config.min_change_interval {
                // if it's been long enough, and the stats indicate needing to change speeds
                // change speeds here

                (self.state_dl.current_bytes, self.state_ul.current_bytes) =
                    get_interface_stats(&self.config, self.down_direction, self.up_direction)?;
                if self.state_dl.current_bytes == -1 || self.state_ul.current_bytes == -1 {
                    warn!(
                        "One or both Netlink stats could not be read. Skipping rate control algorithm"
                    );
                    continue;
                }

                self.update_deltas()?;

                if self.state_dl.deltas.is_empty() || self.state_ul.deltas.is_empty() {
                    warn!("No reflector data available, dropping to minimum rates");
                    self.state_dl.next_rate = self.config.download_min_kbits;
                    self.state_ul.next_rate = self.config.upload_min_kbits;

                    Netlink::set_qdisc_rate(
                        self.state_dl.qdisc,
                        self.state_dl.next_rate as u64,
                        self.config.cake_ack_filter,
                        &self.config.cake_rtt,
                    )?;
                    Netlink::set_qdisc_rate(
                        self.state_ul.qdisc,
                        self.state_ul.next_rate as u64,
                        self.config.cake_ack_filter,
                        &self.config.cake_rtt,
                    )?;

                    self.state_dl.current_rate = self.state_dl.next_rate;
                    self.state_ul.current_rate = self.state_ul.next_rate;
                    continue;
                }

                self.calculate_rate(Direction::Down)?;
                self.calculate_rate(Direction::Up)?;

                let reflectors = self.reflectors_lock.read_anyhow()?;
                let targets: Vec<String> = reflectors.iter().map(|ip| ip.to_string()).collect();
                let target_str = targets.join(", ");

                use std::io::Write;

                let dl_cooldown_left = match self.state_dl.cooldown_until {
                    Some(time) if time > now_t => (time - now_t).as_secs_f64(),
                    _ => 0.0,
                };
                let ul_cooldown_left = match self.state_ul.cooldown_until {
                    Some(time) if time > now_t => (time - now_t).as_secs_f64(),
                    _ => 0.0,
                };

                let dl_cooldown_str = if dl_cooldown_left > 0.0 {
                    format!(" ({:.1}s left)", dl_cooldown_left)
                } else {
                    "".to_string()
                };

                let ul_cooldown_str = if ul_cooldown_left > 0.0 {
                    format!(" ({:.1}s left)", ul_cooldown_left)
                } else {
                    "".to_string()
                };

                if self.state_dl.status == "CONGESTION" {
                    info!("[CONGESTION] DL Target: [{}] | RTT: {:.2}ms | Ceiling: {:.0} Kbps | Dropping to: {} Kbps",
                          target_str, self.state_dl.delta_stat, self.state_dl.congestion_ceiling, self.state_dl.next_rate as u64);
                } else if self.state_dl.status == "PROBING" {
                    info!("[PROBING] DL Target: [{}] | RTT: {:.2}ms | Limit: {} Kbps",
                          target_str, self.state_dl.delta_stat, self.state_dl.next_rate as u64);
                } else if self.state_dl.status == "COOLDOWN" || self.state_dl.status == "HOLD" {
                    info!("[{}] DL Target: [{}] | RTT: {:.2}ms | Holding at: {} Kbps{}",
                          self.state_dl.status, target_str, self.state_dl.delta_stat, self.state_dl.next_rate as u64, dl_cooldown_str);
                }

                if self.state_ul.status == "CONGESTION" {
                    info!("[CONGESTION] UL Target: [{}] | RTT: {:.2}ms | Ceiling: {:.0} Kbps | Dropping to: {} Kbps",
                          target_str, self.state_ul.delta_stat, self.state_ul.congestion_ceiling, self.state_ul.next_rate as u64);
                } else if self.state_ul.status == "PROBING" {
                    info!("[PROBING] UL Target: [{}] | RTT: {:.2}ms | Limit: {} Kbps",
                          target_str, self.state_ul.delta_stat, self.state_ul.next_rate as u64);
                } else if self.state_ul.status == "COOLDOWN" || self.state_ul.status == "HOLD" {
                    info!("[{}] UL Target: [{}] | RTT: {:.2}ms | Holding at: {} Kbps{}",
                          self.state_ul.status, target_str, self.state_ul.delta_stat, self.state_ul.next_rate as u64, ul_cooldown_str);
                }

                let _ = std::io::stdout().flush();

                if self.state_dl.next_rate != self.state_dl.current_rate
                    || self.state_ul.next_rate != self.state_ul.current_rate
                {
                    info!(
                        "self.state_ul.next_rate {} self.state_dl.next_rate {}",
                        self.state_ul.next_rate, self.state_dl.next_rate
                    );
                }

                if self.state_dl.next_rate != self.state_dl.current_rate {
                    Netlink::set_qdisc_rate(self.state_dl.qdisc, self.state_dl.next_rate as u64, self.config.cake_ack_filter, &self.config.cake_rtt)?;
                }

                if self.state_ul.next_rate != self.state_ul.current_rate {
                    Netlink::set_qdisc_rate(self.state_ul.qdisc, self.state_ul.next_rate as u64, self.config.cake_ack_filter, &self.config.cake_rtt)?;
                }

                self.state_dl.current_rate = self.state_dl.next_rate;
                self.state_ul.current_rate = self.state_ul.next_rate;

                let stats_time = Time::new(ClockId::Realtime);
                debug!(
                    "{},{},{},{},{},{},{},{}",
                    stats_time.secs(),
                    stats_time.nsecs(),
                    self.state_dl.load,
                    self.state_ul.load,
                    self.state_dl.delta_stat,
                    self.state_ul.delta_stat,
                    self.state_dl.current_rate as u64,
                    self.state_ul.current_rate as u64
                );

                if let Some(ref mut fd) = stats_fd {
                    if let Err(e) = fd.write_all(
                        format!(
                            "{},{},{},{},{},{},{},{}\n",
                            stats_time.secs(),
                            stats_time.nsecs(),
                            self.state_dl.load,
                            self.state_ul.load,
                            self.state_dl.delta_stat,
                            self.state_ul.delta_stat,
                            self.state_dl.current_rate as u64,
                            self.state_ul.current_rate as u64
                        )
                        .as_bytes(),
                    ) {
                        warn!("Failed to write statistics: {}", e);
                    }
                }

                lastchg_t = now_t;
            }

        }
    }
}
