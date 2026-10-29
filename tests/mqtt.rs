//! Application integration against private Mosquitto brokers, never hardware or public services.
use embedded_io_adapters::tokio_1::FromTokio;
use miniconf_mqtt::{Miniconf, Service, ServiceEvent};
use minimq::{ConfigBuilder, Session};
use std::{cell::Cell, net::SocketAddr, process::Stdio, sync::Once, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, BufReader, Lines},
    net::{TcpListener, TcpStream},
    process::{Child, ChildStdout, Command},
    time::timeout,
};
use xiao_telemetry::{Settings, Shared, Telemetry};

const PREFIX: &str = "test/esp32c6-tele/host-test";

struct Broker {
    _process: Child,
    addr: SocketAddr,
}

impl Broker {
    async fn new() -> Self {
        static LOGGING: Once = Once::new();
        LOGGING.call_once(defmt2log::init_from_current_exe);
        let reservation = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = reservation.local_addr().unwrap();
        drop(reservation);
        let process = Command::new("/usr/sbin/mosquitto")
            .args(["-p", &addr.port().to_string()])
            .stdout(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        timeout(Duration::from_secs(5), async {
            while TcpStream::connect(addr).await.is_err() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        Self {
            _process: process,
            addr,
        }
    }

    fn observe(&self) -> Observer {
        let mut process = Command::new("mosquitto_sub")
            .args([
                "-h",
                "127.0.0.1",
                "-p",
                &self.addr.port().to_string(),
                "-V",
                "mqttv5",
                "-t",
                &format!("{PREFIX}/#"),
                "-F",
                "%t\t%p\t%P",
            ])
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let lines = BufReader::new(process.stdout.take().unwrap()).lines();
        Observer {
            _process: process,
            lines,
        }
    }

    async fn set_interval(&self, value: &str) {
        assert!(
            Command::new("mosquitto_pub")
                .args([
                    "-h",
                    "127.0.0.1",
                    "-p",
                    &self.addr.port().to_string(),
                    "-V",
                    "mqttv5",
                    "-q",
                    "1",
                    "-t",
                    &format!("{PREFIX}/set/interval"),
                    "-m",
                    value,
                    "-D",
                    "PUBLISH",
                    "response-topic",
                    &format!("{PREFIX}/reply")
                ])
                .status()
                .await
                .unwrap()
                .success()
        );
    }
}

struct Observer {
    _process: Child,
    lines: Lines<BufReader<ChildStdout>>,
}

impl Observer {
    // Read an independent wire observation, with a bound on every expected event.
    async fn receive(&mut self, suffix: &str) -> (String, String) {
        timeout(Duration::from_secs(5), async {
            loop {
                let line = self
                    .lines
                    .next_line()
                    .await
                    .unwrap()
                    .expect("subscriber exited");
                let mut fields = line.splitn(3, '\t');
                if fields.next().unwrap() == format!("{PREFIX}/{suffix}") {
                    return (
                        fields.next().unwrap().to_owned(),
                        fields.next().unwrap_or("").to_owned(),
                    );
                }
            }
        })
        .await
        .expect("expected MQTT publication")
    }
}

fn device() -> (Miniconf<Settings>, Session<'static>) {
    Miniconf::new(
        PREFIX,
        ConfigBuilder::from_buffer(Box::leak(Box::new([0; 5120])), 1024)
            .unwrap()
            .client_id(PREFIX)
            .unwrap()
            .keepalive_interval(2),
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "requires /usr/sbin/mosquitto and mosquitto_pub/sub"]
async fn configuration_during_publication_and_idle_keepalive() {
    let broker = Broker::new().await;
    let mut observer = broker.observe();
    // Let the independent client subscribe before startup publications.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (mut miniconf, mut session) = device();
    let io = FromTokio::new(TcpStream::connect(broker.addr).await.unwrap());
    let mut connection = session.connect(io).await.unwrap();
    let mut settings = Settings::default();
    let shared = Shared::new();
    let topic = format!("{PREFIX}/telemetry");
    let producing = Cell::new(None);
    let producer = async {
        loop {
            if let Some(sample) = producing.get() {
                shared.telemetry.signal(sample);
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    };
    let scenario = async {
        observer.receive("alive").await;
        // Configuration must still work after more than two idle keepalive periods.
        tokio::time::sleep(Duration::from_secs(5)).await;
        for value in ["1", "0", "10"] {
            let sample = Telemetry {
                temperature: Some(value.parse().unwrap()),
                rssi: Some(-60),
                ..Telemetry::default()
            };
            producing.set(Some(sample));
            broker.set_interval(value).await;
            if value != "0" {
                assert_eq!(observer.receive("settings/interval").await.0, value);
            }
            let (_, properties) = observer.receive("reply").await;
            let expected = if value == "0" {
                "code:Error"
            } else {
                "code:Ok"
            };
            assert!(
                properties
                    .split_whitespace()
                    .any(|property| property == expected),
                "{properties}"
            );
            let changed = shared.interval.try_take();
            assert_eq!(
                changed.map(|value| value.get()),
                if value == "0" {
                    None
                } else {
                    Some(value.parse().unwrap())
                }
            );
            producing.set(None);
            shared.telemetry.signal(sample);
            loop {
                let (payload, _) = observer.receive("telemetry").await;
                let json: serde_json::Value = serde_json::from_str(&payload).unwrap();
                if json["temperature"].as_f64() == sample.temperature.map(f64::from) {
                    assert!(json["humidity"].is_null());
                    assert_eq!(json["rssi"], -60);
                    break;
                }
            }
        }
    };
    tokio::select! {
        result = xiao_telemetry::mqtt::serve(&mut connection, &mut miniconf, &mut settings, &shared, &topic) => panic!("service ended: {result:?}"),
        _ = producer => panic!("producer ended"),
        result = timeout(Duration::from_secs(20), scenario) => result.unwrap(),
    }
    assert_eq!(settings.interval.get(), 10);
}

#[tokio::test]
#[ignore = "requires /usr/sbin/mosquitto and mosquitto_pub/sub"]
async fn reconnect_republishes_live_settings_and_latest_sample() {
    let broker = Broker::new().await;
    let (mut miniconf, mut session) = device();
    let mut settings = Settings::default();
    let io = FromTokio::new(TcpStream::connect(broker.addr).await.unwrap());
    let mut connection = session.connect(io).await.unwrap();
    miniconf.startup(&mut connection, &settings).await.unwrap();
    let mut observer = broker.observe();
    assert_eq!(observer.receive("settings/interval").await.0, "10");
    broker.set_interval("1").await;
    let mut service = Service::<4>::new();
    timeout(Duration::from_secs(5), async {
        loop {
            if let Some(inbound) = connection.poll().await.unwrap() {
                assert!(matches!(
                    service.handle(&mut miniconf, &mut settings, &inbound),
                    ServiceEvent::Changed(_)
                ));
                break;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(settings.interval.get(), 1);
    // Fail between acceptance and publication, as the firmware can on transport loss.
    drop(service);
    drop(connection);
    let shared = Shared::new();
    for temperature in 11..=20 {
        shared.telemetry.signal(Telemetry {
            temperature: Some(temperature as f32),
            ..Telemetry::default()
        });
    }
    let io = FromTokio::new(TcpStream::connect(broker.addr).await.unwrap());
    let mut connection = session.connect(io).await.unwrap();
    let topic = format!("{PREFIX}/telemetry");
    let scenario = async {
        assert_eq!(observer.receive("settings/interval").await.0, "1");
        let (payload, _) = observer.receive("telemetry").await;
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&payload).unwrap()["temperature"],
            20.0
        );
        // The new connection must also accept configuration changes.
        broker.set_interval("10").await;
        assert_eq!(observer.receive("settings/interval").await.0, "10");
        let (_, properties) = observer.receive("reply").await;
        assert!(properties.split_whitespace().any(|p| p == "code:Ok"));
    };
    tokio::select! {
        result = xiao_telemetry::mqtt::serve(&mut connection, &mut miniconf, &mut settings, &shared, &topic) => panic!("service ended: {result:?}"),
        _ = scenario => {},
    }
    assert_eq!(settings.interval.get(), 10);
}
