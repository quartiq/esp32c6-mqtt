use core::fmt::Write;
use embassy_executor::Spawner;
use embassy_futures::select::select;
use embassy_net::{Runner, StackResources, dns::DnsQueryType, tcp::TcpSocket};
use embassy_time::{Duration, Timer, with_timeout};
use esp_hal::peripherals::WIFI;
use esp_radio::wifi::{
    AuthenticationMethodConfig, ControllerConfig, Interface, WifiController, sta::StationConfig,
};
use miniconf_mqtt::Miniconf;
use minimq::{Buffers, ConfigBuilder};
use static_cell::StaticCell;
use xiao_telemetry::{Settings, Shared};

pub async fn run(spawner: Spawner, radio: WIFI<'static>, seed: u64, shared: &'static Shared) -> ! {
    let station = StationConfig::default()
        .with_ssid(env!("WIFI_SSID").try_into().unwrap())
        .with_authentication(AuthenticationMethodConfig::Wpa2Personal(
            env!("WIFI_PASSWORD").try_into().unwrap(),
        ));
    let controller = WifiController::new(
        radio,
        ControllerConfig::default().with_initial_config(esp_radio::wifi::Config::Station(station)),
    )
    .unwrap();
    let interface = Interface::station();
    let mut prefix = heapless::String::<64>::new();
    write!(prefix, "test/esp32c6-tele/").unwrap();
    for (index, byte) in interface.mac_address().iter().enumerate() {
        if index != 0 {
            prefix.push(':').unwrap();
        }
        write!(prefix, "{byte:02x}").unwrap();
    }
    let mut topic = prefix.clone();
    topic.push_str("/telemetry").unwrap();
    defmt::info!("XIAO C6 Logger HAT prefix={=str}", prefix.as_str());
    static RESOURCES: StaticCell<StackResources<3>> = StaticCell::new();
    let (stack, runner) = embassy_net::new(
        interface,
        embassy_net::Config::dhcpv4(Default::default()),
        RESOURCES.init(StackResources::new()),
        seed,
    );
    spawner.spawn(wifi(controller, shared).unwrap());
    spawner.spawn(network(runner).unwrap());
    let mut rx = [0; 1024];
    let mut tx = [0; 4096];
    let config = ConfigBuilder::new(Buffers::new(&mut rx, &mut tx))
        .client_id(prefix.as_str())
        .unwrap()
        .keepalive_interval(30);
    let (mut miniconf, mut session) = Miniconf::new(prefix.as_str(), config).unwrap();
    let mut settings = Settings::default();
    let mut backoff = 1;
    let host = option_env!("MQTT_HOST").unwrap_or("broker.emqx.io");
    let port = option_env!("MQTT_PORT")
        .unwrap_or("1883")
        .parse::<u16>()
        .unwrap();
    loop {
        stack.wait_config_up().await;
        let mut tcp_rx = [0; 2048];
        let mut tcp_tx = [0; 2048];
        let mut socket = TcpSocket::new(stack, &mut tcp_rx, &mut tcp_tx);
        socket.set_timeout(Some(Duration::from_secs(45)));
        let result = with_timeout(Duration::from_secs(20), async {
            let address = *stack
                .dns_query(host, DnsQueryType::A)
                .await
                .map_err(|error| defmt::warn!("DNS lookup failed: {}", error))?
                .first()
                .ok_or(())?;
            defmt::debug!("MQTT TCP address={}:{}", address, port);
            socket
                .connect((address, port))
                .await
                .map_err(|error| defmt::warn!("TCP connect failed: {}", error))
        })
        .await;
        if matches!(result, Ok(Ok(()))) {
            match with_timeout(Duration::from_secs(20), session.connect(socket)).await {
                Ok(Ok(mut connection)) => {
                    backoff = 1;
                    defmt::info!("MQTT connected");
                    let result = xiao_telemetry::mqtt::serve(
                        &mut connection,
                        &mut miniconf,
                        &mut settings,
                        shared,
                        topic.as_str(),
                    )
                    .await;
                    defmt::warn!("MQTT ended: {:?}", defmt::Debug2Format(&result));
                }
                _ => defmt::warn!("MQTT handshake failed"),
            }
        } else {
            defmt::warn!("MQTT transport unavailable");
        }
        Timer::after_secs(backoff).await;
        backoff = (backoff * 2).min(60);
    }
}

#[embassy_executor::task]
async fn wifi(mut controller: WifiController<'static>, shared: &'static Shared) {
    loop {
        match controller.connect_async().await {
            Ok(_) => {
                defmt::info!("Wi-Fi connected");
                let forward = async {
                    loop {
                        let mut sample = shared.samples.wait().await;
                        sample.rssi = controller.rssi().ok();
                        shared.telemetry.signal(sample);
                    }
                };
                // Reconnecting leaves acquisition running; its signal keeps the latest sample.
                let _ = select(controller.wait_for_disconnect_async(), forward).await;
            }
            Err(_) => defmt::warn!("Wi-Fi connection failed"),
        }
        Timer::after_secs(5).await;
    }
}

#[embassy_executor::task]
async fn network(mut runner: Runner<'static, Interface>) {
    runner.run().await
}
