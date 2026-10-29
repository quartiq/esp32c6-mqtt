#![no_std]

pub mod mqtt;

use core::num::NonZeroU32;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Instant, with_deadline};
use miniconf::Tree;
use serde::Serialize;

#[derive(Clone, Copy, Tree)]
pub struct Settings {
    /// Sampling period in seconds. Changes are volatile.
    #[tree(with = miniconf::leaf, meta(unit = "s", min = "1"))]
    pub interval: NonZeroU32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            interval: NonZeroU32::new(10).unwrap(),
        }
    }
}

#[derive(Clone, Copy, Default, Serialize)]
pub struct Telemetry {
    /// Internal chip temperature in degrees Celsius, not ambient temperature.
    pub chip_temperature: f32,
    /// Degrees Celsius, or null on read failure.
    pub temperature: Option<f32>,
    /// Relative humidity in percent, or null on read failure.
    pub humidity: Option<f32>,
    /// Illuminance in lux, or null on read failure.
    pub illuminance: Option<f32>,
    /// Associated access point's last beacon RSSI in dBm, or null if unavailable.
    pub rssi: Option<i32>,
}

/// Latest-value handoffs between sensor acquisition, Wi-Fi enrichment and MQTT.
#[derive(Default)]
pub struct Shared {
    /// Acquisition to Wi-Fi; RSSI has not been filled yet.
    pub samples: Signal<CriticalSectionRawMutex, Telemetry>,
    /// Wi-Fi to MQTT; ready to serialize.
    pub telemetry: Signal<CriticalSectionRawMutex, Telemetry>,
    pub interval: Signal<CriticalSectionRawMutex, NonZeroU32>,
}

impl Shared {
    pub const fn new() -> Self {
        Self {
            samples: Signal::new(),
            telemetry: Signal::new(),
            interval: Signal::new(),
        }
    }

    /// Wait for the next acquisition, interrupting the wait on interval changes.
    pub async fn next_sample(&self, start: Instant, mut interval: NonZeroU32) -> NonZeroU32 {
        let period = Duration::from_secs(u64::from(interval.get()));
        let mut deadline = start + period;
        if deadline <= Instant::now() {
            deadline = Instant::now() + period;
        }
        while let Ok(next) = with_deadline(deadline, self.interval.wait()).await {
            interval = next;
            deadline = Instant::now() + Duration::from_secs(u64::from(interval.get()));
        }
        interval
    }
}
