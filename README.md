# XIAO Logger HAT → MQTT

Chip temperature and Wi-Fi signal strength from a **Seeed Studio XIAO ESP32-C6**,
plus ambient temperature, humidity and light with a XIAO Logger HAT. Powered by
USB. About 400 lines of embedded Rust combine Embassy, Minimq, Miniconf and
existing sensor drivers: sampling, MQTT publication,
remote configuration and reconnection.

## Build and flash

Install [stable Rust](https://rustup.rs/) and
[espflash](https://github.com/esp-rs/espflash/tree/main/espflash#installation).
Connect the board by USB. From this checkout, on Linux with Bash:

```sh
rustup target add riscv32imac-unknown-none-elf
read -r -p 'Wi-Fi SSID: ' WIFI_SSID
read -r -s -p 'Wi-Fi password: ' WIFI_PASSWORD; echo
export WIFI_SSID WIFI_PASSWORD
cargo build --release --locked
ls /dev/serial/by-id/
read -r -p 'C6 serial device path: ' C6_PORT
espflash flash --chip esp32c6 --flash-size 4mb \
  --port "$C6_PORT" \
  target/riscv32imac-unknown-none-elf/release/xiao-telemetry
unset WIFI_PASSWORD
```

Use a 2.4 GHz WPA2 network and select your C6's serial path.
The default broker is `broker.emqx.io:1883`; set `MQTT_HOST`/`MQTT_PORT`
before building to change it. Wi-Fi credentials are embedded in the resulting
firmware, so keep that image private.

## Watch and configure

1. [Open MQTT Telemetry](https://telemetry.quartiq.de/?broker=wss%3A%2F%2Fbroker.emqx.io%3A8084%2Fmqtt&sub=test%2Fesp32c6-tele%2F%2B%2Ftelemetry&history=1000&window=10m).
   Select your device's topic and pin the readings you want to plot.
2. [Open Miniconf discovery](https://miniconf.quartiq.de/#/discover/wss+broker.emqx.io:8084/test/esp32c6-tele/+?endpoint=%2Fmqtt).
   Select the same device and set `/interval` to `1`. Watch the plots update
   every second; set it back to `10` for the default.

The links use the public EMQX broker: readings and interval control are public.
The interval is the only writable setting, accepts positive integer seconds,
and resets to 10 on reboot. For another broker, change the connection in each
web interface; browsers need its WebSocket endpoint.

## Collect readings

Each device publishes JSON at `test/esp32c6-tele/{mac}/telemetry`, with its
lowercase, colon-separated MAC. Any MQTT client can consume it:

```sh
mosquitto_sub -h broker.emqx.io -V mqttv5 \
  -t 'test/esp32c6-tele/+/telemetry' -v
```

Telemetry contains `chip_temperature` (°C inside the chip), `rssi` (dBm from the
access point's last beacon), and the HAT's `temperature`, `humidity` and
`illuminance` (°C, percent RH and lux). Chip temperature is not ambient temperature.
Missing HAT readings and unavailable RSSI are `null`; no HAT configuration is
needed. Failed sensor reads are retried each sampling period. The interval is
published by Miniconf as a setting. Sampling continues offline, keeping the
latest pending readings. Telemetry uses QoS 0: missed readings are not replayed,
and there is no on-device history. Configuration uses QoS 1; live settings survive
reconnection and are republished. Browser plots use receipt time.

## Read and adapt

Start with [board startup](src/firmware.rs), then [sensors](src/firmware/sensors.rs)
and [networking](src/firmware/network.rs). The reusable example is
[MQTT composition](src/mqtt.rs): Miniconf configuration and telemetry share one
connection while Embassy signals keep sampling independent of network progress.
The sensor task owns I²C and chip temperature; the Wi-Fi task adds RSSI before
handing telemetry to MQTT. RSSI is read when Wi-Fi forwards a sample, including
after reconnection.
[Settings and telemetry](src/lib.rs) define the application contract.

Host tests cover interval changes, serialization, configuration under load and
MQTT recovery. On Linux x86-64:

```sh
cargo test --target x86_64-unknown-linux-gnu --locked -- --test-threads=8
# Include broker tests with /usr/sbin/mosquitto and mosquitto_pub/sub installed:
cargo test --target x86_64-unknown-linux-gnu --locked -- --include-ignored --test-threads=8
```
