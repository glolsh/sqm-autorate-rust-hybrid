use rustix::fs::Timespec;
use rustix::thread::ClockId;
use rustix::time::clock_gettime;

pub struct Time {
    time_s: u64,
    time_ns: u64,
}

impl Time {
    pub fn new(id: ClockId) -> Self {
        let time: Timespec = clock_gettime(id);
        Self {
            time_s: time.tv_sec as u64,
            time_ns: time.tv_nsec as u64,
        }
    }

    pub fn get_time_since_midnight(&self) -> i64 {
        (self.time_s as i64 % 86400 * 1000) + (self.time_ns as i64 / 1000000)
    }

    pub fn secs(&self) -> u64 {
        self.time_s
    }

    pub fn nsecs(&self) -> u64 {
        self.time_ns
    }

    pub fn to_milliseconds(&self) -> u64 {
        (self.time_s * 1000) + (self.time_ns / 1000000)
    }
}

const MS_PER_DAY: i64 = 86_400_000;

/// Difference of two "milliseconds since midnight" timestamps, taking the
/// midnight wrap-around into account. The result is in (-12h, +12h].
pub fn wrapped_diff_ms(a: i64, b: i64) -> i64 {
    let d = (a - b).rem_euclid(MS_PER_DAY);
    if d > MS_PER_DAY / 2 { d - MS_PER_DAY } else { d }
}

#[cfg(test)]
mod tests {
    use super::wrapped_diff_ms;

    #[test]
    fn diff_without_wrap() {
        assert_eq!(wrapped_diff_ms(1_050, 1_000), 50);
        assert_eq!(wrapped_diff_ms(1_000, 1_050), -50);
    }

    #[test]
    fn diff_across_midnight() {
        // sent just before midnight, received just after
        assert_eq!(wrapped_diff_ms(10, 86_400_000 - 20), 30);
        assert_eq!(wrapped_diff_ms(86_400_000 - 20, 10), -30);
    }
}
