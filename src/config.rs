use anyhow::Result;
#[cfg(feature = "uci")]
use log::warn;
#[cfg(feature = "uci")]
use rust_uci::Uci;
use std::net::IpAddr;
use std::str::FromStr;
use std::env;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Invalid measurement type")]
    InvalidMeasurementType(String),
    #[error("Couldn't parse value for key: `{0}`: invalid value")]
    ParseError(String),
    #[error("No config value found for key: `{0}`")]
    MissingValue(String),
}

#[derive(Clone, Copy, Debug)]
pub enum MeasurementType {
    Icmp = 1,
    IcmpTimestamps,
    Ntp,
    TcpTimestamps,
}

impl FromStr for MeasurementType {
    type Err = ConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        return match s.to_lowercase().as_str() {
            "icmp" => Ok(MeasurementType::Icmp),
            "icmp-timestamps" => Ok(MeasurementType::IcmpTimestamps),
            "ntp" => Ok(MeasurementType::Ntp),
            "tcp-timestamps" => Ok(MeasurementType::TcpTimestamps),
            &_ => Err(ConfigError::InvalidMeasurementType(s.to_string())),
        };
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    // Network section
    pub download_interface: String,
    pub upload_interface: String,
    pub download_base_kbits: f64,
    pub download_max_kbits: f64,
    pub download_min_kbits: f64,
    pub upload_base_kbits: f64,
    pub upload_max_kbits: f64,
    pub upload_min_kbits: f64,

    // Output section
    pub stats_file: String,
    pub suppress_statistics: bool,

    // Advanced section
    pub download_delay_ms: f64,
    pub high_load_level: f64,
    pub min_change_interval: f64,
    pub measurement_type: MeasurementType,
    pub num_reflectors: u8,
    pub peer_reselection_time: u64,
    pub reflectors: String,
    pub tick_interval: f64,
    pub upload_delay_ms: f64,

    // AIMD Rate Control
    pub increase_step_kbits: f64,
    pub decrease_multiplier: f64,
    pub cooldown_secs: f64,
}

impl Config {
    pub fn new() -> Result<Self> {
        let config = Self {
            // Network section
            download_base_kbits: Self::get::<f64>(
                "SQMA_DOWNLOAD_BASE_KBITS",
                "sqm-autorate.main.download_base_kbits",
                None,
            )?,
            download_interface: Self::get::<String>(
                "SQMA_DOWNLOAD_INTERFACE",
                "sqm-autorate.main.download_interface",
                None,
            )?,
            download_max_kbits: Self::get::<f64>(
                "SQMA_DOWNLOAD_MAX_KBITS",
                "sqm-autorate.main.download_max_kbits",
                Some(0.0),
            )?,
            download_min_kbits: Self::get::<f64>(
                "SQMA_DOWNLOAD_MIN_KBITS",
                "sqm-autorate.main.download_min_kbits",
                Some(1000.0),
            )?,
            upload_base_kbits: Self::get::<f64>(
                "SQMA_UPLOAD_BASE_KBITS",
                "sqm-autorate.main.upload_base_kbits",
                None,
            )?,
            upload_interface: Self::get::<String>(
                "SQMA_UPLOAD_INTERFACE",
                "sqm-autorate.main.upload_interface",
                None,
            )?,
            upload_max_kbits: Self::get::<f64>(
                "SQMA_UPLOAD_MAX_KBITS",
                "sqm-autorate.main.upload_max_kbits",
                Some(0.0),
            )?,
            upload_min_kbits: Self::get::<f64>(
                "SQMA_UPLOAD_MIN_KBITS",
                "sqm-autorate.main.upload_min_kbits",
                Some(1000.0),
            )?,
            // Output section
            stats_file: Self::get::<String>(
                "SQMA_STATS_FILE",
                "sqm-autorate.main.stats_file",
                Some("/tmp/sqm-autorate.csv".parse()?),
            )?,
            suppress_statistics: Self::get::<bool>(
                "SQMA_SUPPRESS_STATISTICS",
                "sqm-autorate.main.suppress_statistics",
                Some(false),
            )?,
            // Advanced section
            download_delay_ms: Self::get::<f64>(
                "SQMA_DOWNLOAD_DELAY_MS",
                "sqm-autorate.main.download_delay_ms",
                Some(20.0),
            )?,
            high_load_level: Self::get::<f64>(
                "SQMA_HIGH_LOAD_LEVEL",
                "sqm-autorate.main.high_load_level",
                Some(0.5),
            )?,
            measurement_type: Self::get::<MeasurementType>(
                "SQMA_MEASUREMENT_TYPE",
                "sqm-autorate.main.measurement_type",
                Some(MeasurementType::IcmpTimestamps),
            )?,
            min_change_interval: Self::get::<f64>(
                "SQMA_MIN_CHANGE_INTERVAL",
                "sqm-autorate.main.min_change_interval",
                Some(0.5),
            )?,
            num_reflectors: Self::get::<u8>(
                "SQMA_NUM_REFLECTORS",
                "sqm-autorate.main.num_reflectors",
                Some(5),
            )?,
            peer_reselection_time: Self::get::<u64>(
                "SQMA_PEER_RESELECTION_TIME",
                "sqm-autorate.main.peer_reselection_time",
                Some(15),
            )?,
            reflectors: Self::get::<String>(
                "SQMA_REFLECTORS",
                "sqm-autorate.main.reflectors",
                Some("".parse()?),
            )?,
            tick_interval: Self::get::<f64>(
                "SQMA_TICK_INTERVAL",
                "sqm-autorate.main.tick_interval",
                Some(0.5),
            )?,
            upload_delay_ms: Self::get::<f64>(
                "SQMA_UPLOAD_DELAY_MS",
                "sqm-autorate.main.upload_delay_ms",
                Some(20.0),
            )?,
            increase_step_kbits: Self::get::<f64>(
                "SQMA_INCREASE_STEP_KBITS",
                "sqm-autorate.main.increase_step_kbits",
                Some(5000.0),
            )?,
            decrease_multiplier: Self::get::<f64>(
                "SQMA_DECREASE_MULTIPLIER",
                "sqm-autorate.main.decrease_multiplier",
                Some(0.85),
            )?,
            cooldown_secs: Self::get::<f64>(
                "SQMA_COOLDOWN_SECS",
                "sqm-autorate.main.cooldown_secs",
                Some(10.0),
            )?,
        };

        let mut config = config;

        if config.download_base_kbits == 0.0 {
            config.download_base_kbits = 10_000_000.0;
        }
        if config.upload_base_kbits == 0.0 {
            config.upload_base_kbits = 10_000_000.0;
        }

        Ok(config)
    }

    fn get<T: FromStr>(env_key: &str, uci_key: &str, default: Option<T>) -> Result<T, ConfigError> {
        match Self::get_value(env_key, uci_key) {
            Some(val) => {
                // Special handling for booleans in OpenWrt's UCI system
                if std::any::type_name::<T>() == "bool" {
                    let v = val.trim().to_lowercase();
                    if v == "1" || v == "true" || v == "on" || v == "yes" {
                        return "true".parse::<T>().map_err(|_| ConfigError::ParseError(env_key.to_string()));
                    } else if v == "0" || v == "false" || v == "off" || v == "no" {
                        return "false".parse::<T>().map_err(|_| ConfigError::ParseError(env_key.to_string()));
                    }
                }
                match val.parse::<T>() {
                    Ok(parsed_val) => Ok(parsed_val),
                    // Ran into an compilation error while trying to return the
                    // error as-is, so using my own error type to indicate something went wrong while parsing
                    Err(_) => Err(ConfigError::ParseError(env_key.to_string())),
                }
            },
            None => match default {
                Some(val) => Ok(val),
                None => Err(ConfigError::MissingValue(env_key.to_string())),
            },
        }
    }

    fn get_value(env_key: &str, uci_key: &str) -> Option<String> {
        if let Ok(val) = env::var(env_key) {
            return Some(val);
        }

        if let Some(val) = Self::get_from_uci(uci_key) {
            return Some(val);
        }

        None
    }

    #[cfg(feature = "uci")]
    fn get_from_uci(key: &str) -> Option<String> {
        let mut uci = match Uci::new() {
            Ok(val) => val,
            Err(e) => {
                warn!("Error opening UCI instance: {}", e);
                return None;
            }
        };

        return match uci.get(key) {
            Ok(val) => Some(val),
            Err(e) => {
                warn!("Problem getting config from UCI: {}", e);
                None
            }
        };
    }

    #[cfg(not(feature = "uci"))]
    fn get_from_uci(_: &str) -> Option<String> {
        None
    }

    pub fn load_reflectors(&self) -> Result<Vec<IpAddr>> {
        let mut reflectors: Vec<IpAddr> = Vec::with_capacity(50);

        for ip_str in self.reflectors.split(|c| c == ' ' || c == ',') {
            let ip_str = ip_str.trim();
            if !ip_str.is_empty() {
                reflectors.push(IpAddr::from_str(ip_str)?);
            }
        }

        Ok(reflectors)
    }
}
