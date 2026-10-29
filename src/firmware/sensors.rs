use embassy_embedded_hal::shared_bus::asynch::i2c::I2cDevice;
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, mutex::Mutex};
use embassy_time::{Instant, Timer};
use esp_hal::{Async, i2c::master::I2c, tsens::TemperatureSensor};
use xiao_telemetry::{Settings, Shared, Telemetry};

#[embassy_executor::task]
pub async fn run(
    i2c: I2c<'static, Async>,
    temperature: TemperatureSensor<'static>,
    shared: &'static Shared,
) {
    let bus = Mutex::<NoopRawMutex, _>::new(i2c);
    let mut sht = sht4x::Sht4xAsync::new(I2cDevice::new(&bus));
    let mut light = bh1750_embedded::r#async::Bh1750Async::new(
        I2cDevice::new(&bus),
        embassy_time::Delay,
        bh1750_embedded::Address::Low,
    );
    let mut interval = Settings::default().interval;
    loop {
        let start = Instant::now();
        let mut sample = Telemetry {
            chip_temperature: temperature.get_temperature().to_celsius(),
            ..Telemetry::default()
        };
        if let Ok(value) = sht
            .measure(sht4x::Precision::High, &mut embassy_time::Delay)
            .await
        {
            sample.temperature = Some(value.temperature_celsius().to_num());
            sample.humidity = Some(value.humidity_percent().to_num());
        }
        // The crate's one-shot delay (144 ms) is shorter than the 180 ms maximum.
        if light
            .start_continuous_measurement(bh1750_embedded::Resolution::High)
            .await
            .is_ok()
        {
            Timer::after_millis(180).await;
            sample.illuminance = light
                .current_measurement_lux(bh1750_embedded::Resolution::High)
                .await
                .ok();
            let _ = light.power_down().await;
        }
        shared.samples.signal(sample);
        interval = shared.next_sample(start, interval).await;
    }
}
