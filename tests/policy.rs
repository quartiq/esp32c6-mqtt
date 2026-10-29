use defmt2log as _;
use embassy_time::Instant;
use xiao_telemetry::{Shared, Telemetry};

#[test]
fn telemetry_wire_format() {
    let telemetry = Telemetry {
        chip_temperature: 35.0,
        temperature: Some(21.5),
        humidity: Some(48.0),
        illuminance: None,
        rssi: Some(-60),
    };
    let mut buffer = [0; 512];
    let len = serde_json_core::to_slice(&telemetry, &mut buffer).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&buffer[..len]).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "chip_temperature": 35.0, "temperature": 21.5,
            "humidity": 48.0, "illuminance": null, "rssi": -60
        })
    );
}

#[tokio::test]
async fn update_interrupts_long_wait_including_change_during_conversion() {
    let shared = Shared::new();
    // A change received during acquisition remains pending when scheduling starts.
    shared.interval.signal(1.try_into().unwrap());
    let now = std::time::Instant::now();
    let waiting = shared.next_sample(Instant::now(), u32::MAX.try_into().unwrap());
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), waiting)
        .await
        .unwrap();
    assert_eq!(result.get(), 1);
    assert!(now.elapsed() >= std::time::Duration::from_millis(950));

    let waiting = shared.next_sample(Instant::now(), u32::MAX.try_into().unwrap());
    let change = async {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        shared.interval.signal(1.try_into().unwrap());
    };
    let (result, _) = tokio::join!(
        tokio::time::timeout(std::time::Duration::from_secs(2), waiting),
        change
    );
    assert_eq!(result.unwrap().get(), 1);
}

#[tokio::test]
async fn missed_deadline_does_not_cause_burst() {
    let shared = Shared::new();
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let now = std::time::Instant::now();
    shared
        .next_sample(Instant::from_ticks(0), 1.try_into().unwrap())
        .await;
    assert!(now.elapsed() >= std::time::Duration::from_millis(950));
}
