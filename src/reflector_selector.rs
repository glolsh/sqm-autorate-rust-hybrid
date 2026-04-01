use crate::util::{MutexExt, RwLockExt};
use crate::{Config, ReflectorStats};
use log::{debug, info, warn};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, RwLock};
use std::thread::sleep;
use std::time::{Duration, Instant};

pub struct ReflectorSelector {
    pub config: Config,
    pub owd_recent: Arc<Mutex<HashMap<IpAddr, ReflectorStats>>>,
    pub reflector_peers_lock: Arc<RwLock<Vec<IpAddr>>>,
    pub reflector_pool: Vec<IpAddr>,
    pub trigger_channel: Receiver<bool>,
}

impl ReflectorSelector {
    pub fn run(&self) -> anyhow::Result<()> {
        let mut selector_sleep_time = Duration::new(30, 0);
        let mut reselection_count = 0;
        let baseline_sleep_time =
            Duration::from_secs_f64(self.config.tick_interval * std::f64::consts::PI);

        // Initial wait of several seconds to allow some OWD data to build up
        sleep(baseline_sleep_time);

        let mut penalty_box: std::collections::HashMap<std::net::IpAddr, (u32, std::time::Instant)> = std::collections::HashMap::new();

        loop {
            /*
             * Selection is triggered either by some other thread triggering it through the channel,
             * or it passes the timeout. In any case we don't care about the result of this function,
             * so we ignore the result of it.
             */
            let _ = self
                .trigger_channel
                .recv_timeout(selector_sleep_time)
                .unwrap_or(true);
            reselection_count += 1;
            info!("Starting reselection [#{}]", reselection_count);

            // After 40 reselections, slow down to every 15 minutes
            if reselection_count > 40 {
                selector_sleep_time = Duration::new(self.config.peer_reselection_time * 60, 0);
            }

            let mut next_peers: Vec<IpAddr> = Vec::new();
            let mut reflectors_peers = self.reflector_peers_lock.write_anyhow()?;

            // Include all current peers
            for reflector in reflectors_peers.iter() {
                debug!("Current peer: {}", reflector.to_string());
                next_peers.push(*reflector);
            }

            for _ in 0..20 {
                let next_candidate = &self.reflector_pool[fastrand::usize(..self.reflector_pool.len())];
                if next_peers.contains(next_candidate) {
                    continue;
                }
                if let Some((fail_count, last_check_time)) = penalty_box.get(next_candidate) {
                    let penalty_duration = match *fail_count {
                        0 => 0,
                        1 => 300, // 5 minutes
                        2 => 1800, // 30 minutes
                        n => std::cmp::min(3600 * 2u64.pow(n.saturating_sub(3) as u32), 43200), // capped at 12 hours
                    };
                    if Instant::now().duration_since(*last_check_time).as_secs() < penalty_duration {
                        continue;
                    }
                }
                debug!("Next candidate: {}", next_candidate.to_string());
                next_peers.push(*next_candidate);
            }

            // Clone next_peers because we need it again after the baseline sleep
            // to iterate over candidates for RTT measurement.
            *reflectors_peers = next_peers.clone();

            // Drop the MutexGuard explicitly, as Rust won't unlock the mutex by default
            // until the guard goes out of scope
            drop(reflectors_peers);

            debug!("Waiting for candidates to be baselined");
            // Wait for several seconds to allow all reflectors to be re-baselined
            sleep(baseline_sleep_time);

            // Re-acquire the lock when we wake up again
            reflectors_peers = self.reflector_peers_lock.write_anyhow()?;

            let mut candidates = Vec::new();
            let owd_recent = self.owd_recent.lock_anyhow()?;

            for peer in next_peers {
                if owd_recent.contains_key(&peer) && Instant::now().duration_since(owd_recent[&peer].last_receive_time_s).as_secs() < 30 {
                    penalty_box.remove(&peer);
                    let rtt = (owd_recent[&peer].down_ewma + owd_recent[&peer].up_ewma) as u64;
                    candidates.push((peer, rtt));
                    debug!("Candidate reflector: {} RTT: {}", peer.to_string(), rtt);
                } else {
                    let grace_period_active = penalty_box.get(&peer).map_or(false, |(_, last_check_time)| {
                        Instant::now().duration_since(*last_check_time).as_secs() < 10
                    });

                    if !grace_period_active {
                        let entry = penalty_box.entry(peer).or_insert((0, Instant::now()));
                        entry.0 += 1;
                        entry.1 = Instant::now();
                        warn!("Peer {} unresponsive. Fail count: {}. Next retry delayed.", peer, entry.0);
                    }

                    info!(
                        "No data found from candidate reflector: {} - skipping",
                        peer.to_string()
                    );
                }
            }

            // Sort the candidates table now by ascending RTT
            candidates.sort_by(|a, b| a.1.cmp(&b.1));

            // Now we will just limit the candidates down to 2 * num_reflectors
            let mut num_reflectors = self.config.num_reflectors;
            let candidate_pool_num = (2 * num_reflectors) as usize;
            candidates.truncate(candidate_pool_num);

            for (candidate, rtt) in candidates.iter() {
                debug!("Fastest candidate {}: {}", candidate, rtt);
            }

            // Shuffle the deck so we avoid overwhelming good reflectors (Fisher-Yates)
            for i in (1_usize..candidates.len()).rev() {
                let j = fastrand::usize(0..=i);
                candidates.swap(i, j);
            }

            if (candidates.len() as u8) < num_reflectors {
                num_reflectors = candidates.len() as u8;
            }

            let mut new_peers = Vec::new();
            for i in 0..num_reflectors {
                new_peers.push(candidates[i as usize].0);
                debug!(
                    "New selected peer: {}",
                    candidates[i as usize].0.to_string()
                );
            }

            *reflectors_peers = new_peers;
        }
    }
}
