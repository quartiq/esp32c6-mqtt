mod network;
mod sensors;

use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_time::Timer;
use esp_hal::{
    gpio::{Level, Output, OutputConfig},
    i2c::master::{Config, I2c, SoftwareTimeout},
    rng::{Trng, TrngSource},
    timer::timg::TimerGroup,
    tsens::TemperatureSensor,
};
use xiao_telemetry::Shared;

esp_bootloader_esp_idf::esp_app_desc!();
static SHARED: Shared = Shared::new();

#[esp_hal::main]
async fn main(spawner: Spawner) -> ! {
    let p = esp_hal::init(esp_hal::Config::default());
    esp_alloc::heap_allocator!(size: 96 * 1024);
    let seed = {
        let _entropy = TrngSource::new(p.RNG, p.ADC1);
        let rng = Trng::try_new().unwrap();
        (u64::from(rng.random()) << 32) | u64::from(rng.random())
    };
    let timers = TimerGroup::new(p.TIMG0);
    esp_rtos::start(timers.timer0, p.FROM_CPU_INTR0);
    let _power = Output::new(p.GPIO18, Level::High, OutputConfig::default());
    let _antenna_enable = Output::new(p.GPIO3, Level::Low, OutputConfig::default());
    let _antenna = Output::new(p.GPIO14, Level::Low, OutputConfig::default());
    let temperature = TemperatureSensor::new(p.TSENS, Default::default()).unwrap();
    // Also allows the chip temperature sensor to stabilize.
    Timer::after_millis(100).await;
    let i2c = I2c::new(
        p.I2C0,
        Config::default().with_software_timeout(SoftwareTimeout::Transaction(
            esp_hal::time::Duration::from_millis(100),
        )),
    )
    .unwrap()
    .with_sda(p.GPIO22)
    .with_scl(p.GPIO23)
    .into_async();
    spawner.spawn(sensors::run(i2c, temperature, &SHARED).unwrap());
    network::run(spawner, p.WIFI, seed, &SHARED).await
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    defmt::error!("panic: {}", defmt::Display2Format(info));
    loop {
        core::hint::spin_loop();
    }
}
