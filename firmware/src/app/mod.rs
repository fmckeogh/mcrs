//! The repeater, with Wi-Fi, OTA updates and optional MQTT reporting layered on top.
pub mod ota;
pub mod telnet;

#[cfg(feature = "mqtt")]
pub use mcrs_firmware::mqtt;
pub use mcrs_repeater::{cli, discovery, handler_loop, periodic, radio_loop};

pub mod config {
    #[cfg(feature = "mqtt")]
    pub use mcrs_firmware::{mqtt::MqttConfig, network::MQTT_SERVER_COUNT};

    pub type AppConfig = mcrs_repeater::config::AppConfig<mcrs_firmware::network::NetworkConfig>;
}

use alloc::{format, string::String};
#[cfg(feature = "mqtt")]
use core::{fmt::Write as _, sync::atomic::AtomicU8, sync::atomic::Ordering};
use core::{
    cell::{Cell, RefCell},
    task::Waker,
};
#[cfg(feature = "mqtt")]
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
#[cfg(feature = "mqtt")]
use mcrs_firmware::network::{MQTT_SERVER_COUNT, parse_mqtt_key};
use mcrs_firmware::network::NetworkConfig;
use mcrs_repeater::{
    ConfigUpdateError,
    cli::{CliRequest, denied_text},
    config::ConfigError,
};

use crate::platform::storage::Storage;

pub type AppContext<S> = mcrs_repeater::AppContext<S, Network>;

/// Network state shared between the CLI and the Wi-Fi tasks.
pub struct Network {
    ota_requested: Cell<bool>,
    ota_address: Cell<Option<core::net::SocketAddrV4>>,
    ota_waker: RefCell<Option<Waker>>,
    ota_generation: Cell<u32>,
    #[cfg(feature = "mqtt")]
    mqtt: [Channel<CriticalSectionRawMutex, mqtt::PacketEvent, 8>; MQTT_SERVER_COUNT],
    #[cfg(feature = "mqtt")]
    mqtt_generation: Cell<u32>,
    #[cfg(feature = "mqtt")]
    mqtt_wakers: [RefCell<Option<Waker>>; MQTT_SERVER_COUNT],
    #[cfg(feature = "mqtt")]
    mqtt_states: [AtomicU8; MQTT_SERVER_COUNT],
    #[cfg(feature = "mqtt")]
    mqtt_errors: [Cell<Option<mqtt::ConnectionError>>; MQTT_SERVER_COUNT],
}

impl Network {
    pub fn new() -> Self {
        Self {
            ota_requested: Cell::new(false),
            ota_address: Cell::new(None),
            ota_waker: RefCell::new(None),
            ota_generation: Cell::new(0),
            #[cfg(feature = "mqtt")]
            mqtt: core::array::from_fn(|_| Channel::new()),
            #[cfg(feature = "mqtt")]
            mqtt_generation: Cell::new(0),
            #[cfg(feature = "mqtt")]
            mqtt_wakers: core::array::from_fn(|_| RefCell::new(None)),
            #[cfg(feature = "mqtt")]
            mqtt_states: core::array::from_fn(|_| {
                AtomicU8::new(mqtt::ConnectionState::Disconnected as u8)
            }),
            #[cfg(feature = "mqtt")]
            mqtt_errors: core::array::from_fn(|_| Cell::new(None)),
        }
    }

    pub fn request_ota_start(&self) {
        self.ota_requested.set(true);
        self.wake_ota();
    }

    pub fn request_ota_stop(&self) {
        self.ota_requested.set(false);
        self.wake_ota();
    }

    pub fn ota_requested(&self) -> bool {
        self.ota_requested.get()
    }

    pub fn ota_address(&self) -> Option<core::net::SocketAddrV4> {
        self.ota_address.get()
    }

    pub fn set_ota_address(&self, address: Option<core::net::SocketAddrV4>) {
        self.ota_address.set(address);
    }

    pub fn ota_generation(&self) -> u32 {
        self.ota_generation.get()
    }

    pub fn register_ota_waker(&self, waker: &Waker) {
        self.ota_waker.borrow_mut().replace(waker.clone());
    }

    fn wake_ota(&self) {
        self.ota_generation
            .set(self.ota_generation.get().wrapping_add(1));
        if let Some(waker) = self.ota_waker.borrow_mut().take() {
            waker.wake();
        }
    }

    #[cfg(feature = "mqtt")]
    pub async fn receive_mqtt_packet(&self, index: usize) -> mqtt::PacketEvent {
        self.mqtt[index].receive().await
    }

    #[cfg(feature = "mqtt")]
    pub fn request_mqtt_restart(&self) {
        self.mqtt_generation
            .set(self.mqtt_generation.get().wrapping_add(1));
        for error in &self.mqtt_errors {
            error.set(None);
        }
        for waker in &self.mqtt_wakers {
            if let Some(waker) = waker.borrow_mut().take() {
                waker.wake();
            }
        }
    }

    #[cfg(feature = "mqtt")]
    pub fn mqtt_generation(&self) -> u32 {
        self.mqtt_generation.get()
    }

    #[cfg(feature = "mqtt")]
    pub fn register_mqtt_waker(&self, index: usize, waker: &Waker) {
        if let Some(slot) = self.mqtt_wakers.get(index) {
            slot.borrow_mut().replace(waker.clone());
        }
    }

    #[cfg(feature = "mqtt")]
    pub fn set_mqtt_state(&self, index: usize, state: mqtt::ConnectionState) {
        if matches!(
            state,
            mqtt::ConnectionState::Connected | mqtt::ConnectionState::Disabled
        ) {
            self.set_mqtt_error(index, None);
        }
        if let Some(current) = self.mqtt_states.get(index) {
            current.store(state as u8, Ordering::Relaxed);
        }
    }

    #[cfg(feature = "mqtt")]
    pub fn mqtt_state(&self, index: usize) -> mqtt::ConnectionState {
        self.mqtt_states
            .get(index)
            .map(|state| mqtt::ConnectionState::from_u8(state.load(Ordering::Relaxed)))
            .unwrap_or(mqtt::ConnectionState::Disabled)
    }

    #[cfg(feature = "mqtt")]
    pub fn set_mqtt_error(&self, index: usize, error: Option<mqtt::ConnectionError>) {
        if let Some(slot) = self.mqtt_errors.get(index) {
            slot.set(error);
        }
    }

    #[cfg(feature = "mqtt")]
    pub fn mqtt_error(&self, index: usize) -> Option<mqtt::ConnectionError> {
        self.mqtt_errors.get(index).and_then(Cell::get)
    }

    fn handle_ota_command(&self, command: &str, request: CliRequest) -> String {
        let mut parts = command.split_ascii_whitespace();
        let _ota = parts.next();
        match parts.next() {
            None | Some("status") => ota_status_text(),
            Some("start") => {
                if !request.is_passworded() {
                    denied_text()
                } else {
                    self.request_ota_start();
                    match self.ota_address() {
                        Some(address) => format!("OTA: start requested; http://{}/", address),
                        None => String::from("OTA: start requested; waiting for Wi-Fi"),
                    }
                }
            }
            Some("stop") => {
                if !request.is_passworded() {
                    denied_text()
                } else {
                    self.request_ota_stop();
                    String::from("OTA: stop requested")
                }
            }
            _ => String::from("Err - use 'ota status', 'ota start', or 'ota stop'"),
        }
    }

    #[cfg(feature = "mqtt")]
    fn mqtt_status_text(&self) -> String {
        let mut output = String::new();
        for index in 0..MQTT_SERVER_COUNT {
            let state = self.mqtt_state(index);
            let _ = write!(&mut output, "MQTT {}: {}", index + 1, state.as_str());
            if let Some(error) = self.mqtt_error(index) {
                let _ = write!(&mut output, " (last error: {})", error.as_str());
            }
            output.push('\n');
        }
        output
    }
}

impl mcrs_repeater::Extension for Network {
    type Config = NetworkConfig;

    #[cfg(feature = "mqtt")]
    fn observe_packet(
        &self,
        direction: mcrs_repeater::radio::Direction,
        payload: &[u8],
        rssi: i16,
        snr_quarters: i16,
    ) {
        for channel in &self.mqtt {
            let _ = channel.try_send(mqtt::PacketEvent::new(
                direction,
                payload,
                rssi,
                snr_quarters,
            ));
        }
    }

    async fn handle_command<S>(
        &self,
        context: &AppContext<S>,
        command: &str,
        request: CliRequest,
    ) -> Option<String>
    where
        S: Storage,
    {
        if command == "ota" || command.starts_with("ota ") {
            return Some(self.handle_ota_command(command, request));
        }

        #[cfg(feature = "mqtt")]
        if command == "mqtt status" {
            return Some(if request.is_local() {
                self.mqtt_status_text()
            } else {
                denied_text()
            });
        }

        #[cfg(feature = "mqtt")]
        if command == "mqtt restart" {
            return Some(if request.is_local() && request.is_passworded() {
                self.request_mqtt_restart();
                String::from("OK - MQTT restarting")
            } else {
                denied_text()
            });
        }

        if let Some(setting) = command.strip_prefix("get ").map(str::trim) {
            return get_setting(setting, context, request).await;
        }
        if let Some(setting) = command.strip_prefix("set ").map(str::trim) {
            return set_setting(setting, context, request).await;
        }
        if let Some(setting) = command.strip_prefix("unset ").map(str::trim) {
            return unset_setting(setting, context, request).await;
        }
        None
    }

    fn help_commands(&self) -> &'static str {
        ", ota status"
    }

    fn help_privileged_commands(&self) -> &'static str {
        ", ota {start|stop}"
    }
}

async fn get_setting<S: Storage>(
    setting: &str,
    context: &AppContext<S>,
    request: CliRequest,
) -> Option<String> {
    #[cfg(feature = "mqtt")]
    if let Some((index, key)) = parse_mqtt_key(setting) {
        if !request.is_local() {
            return Some(denied_text());
        }
        return Some(
            context
                .with_config(|config| {
                    config
                        .extension()
                        .mqtt(index)
                        .and_then(|mqtt| mqtt.value(key))
                        .map(|value| format!("> {value}"))
                        .unwrap_or_else(|| format!("Unknown config: {setting}"))
                })
                .await,
        );
    }
    if let Some(key) = setting.strip_prefix("wifi.") {
        if !request.is_local() {
            return Some(denied_text());
        }
        return Some(
            context
                .with_config(|config| {
                    let wifi = config.extension().wifi();
                    match key {
                        "ssid" => format!("> {}", wifi.ssid()),
                        "pass" => format!("> {}", wifi.password()),
                        "telnet" => format!("> {}", cli::on_off_text(wifi.telnet())),
                        _ => format!("Unknown config: wifi.{key}"),
                    }
                })
                .await,
        );
    }
    if setting == "ntp" {
        return Some(
            context
                .with_config(|config| format!("> {}", config.extension().ntp()))
                .await,
        );
    }
    None
}

async fn set_setting<S: Storage>(
    setting: &str,
    context: &AppContext<S>,
    request: CliRequest,
) -> Option<String> {
    let is_ntp = setting == "ntp" || setting.starts_with("ntp ");
    let is_mqtt = cfg!(feature = "mqtt") && setting.starts_with("mqtt.");
    if !is_ntp && !is_mqtt && !setting.starts_with("wifi.") {
        return None;
    }
    if !request.is_passworded() {
        return Some(denied_text());
    }

    if is_ntp {
        let host = setting["ntp".len()..].trim();
        return Some(
            match context
                .update_config(|config| config.extension_mut().set_ntp(host))
                .await
            {
                Ok(()) => String::from("OK - applies at next NTP sync"),
                Err(error) => format!("Error: {error}"),
            },
        );
    }

    if !request.is_local() {
        return Some(denied_text());
    }

    #[cfg(feature = "mqtt")]
    if let Some(mqtt_setting) = setting.strip_prefix("mqtt.") {
        let Some((key, value)) = mqtt_setting.split_once(' ') else {
            return Some(String::from("Error, missing MQTT value"));
        };
        let full_key = format!("mqtt.{}", key.trim());
        let Some((index, field)) = parse_mqtt_key(&full_key) else {
            return Some(String::from(
                "Error, invalid MQTT setting (use mqtt.1 through mqtt.3)",
            ));
        };
        // The command line is trimmed before parsing, so an explicit empty
        // string is needed to clear credentials or an audience override.
        let value = match value.trim() {
            "\"\"" => "",
            value => value,
        };
        return Some(
            match context
                .update_config(|config| config.extension_mut().set_mqtt_value(index, field, value))
                .await
            {
                Ok(()) => String::from("OK - run 'mqtt restart' to apply"),
                Err(error) => format!("Error: {error}"),
            },
        );
    }

    let wifi_setting = setting.strip_prefix("wifi.")?;
    let (key, value) = match wifi_setting.split_once(' ') {
        Some((key, value)) => (key.trim(), value.trim()),
        None if wifi_setting.trim() == "pass" => ("pass", ""),
        None => return Some(String::from("Error, missing Wi-Fi value")),
    };
    let result = match key {
        "ssid" => {
            context
                .update_config(|config| config.extension_mut().set_wifi_ssid(value))
                .await
        }
        "pass" => {
            context
                .update_config(|config| config.extension_mut().set_wifi_password(value))
                .await
        }
        "telnet" => {
            let Some(enabled) = cli::parse_bool(value) else {
                return Some(String::from("Error, invalid wifi.telnet value"));
            };
            context
                .update_config(|config| {
                    config.extension_mut().set_wifi_telnet(enabled);
                    Ok(())
                })
                .await
        }
        _ => return Some(String::from("Error, unknown Wi-Fi setting")),
    };
    Some(match result {
        Ok(()) => String::from("OK - reboot to apply"),
        Err(error) => format!("Error: {error}"),
    })
}

async fn unset_setting<S: Storage>(
    setting: &str,
    context: &AppContext<S>,
    request: CliRequest,
) -> Option<String> {
    let local_only = setting.starts_with("wifi.") || setting.starts_with("mqtt.");
    if !local_only && setting != "ntp" {
        return None;
    }
    if !request.is_passworded() || (local_only && !request.is_local()) {
        return Some(denied_text());
    }

    Some(
        match context.update_config(|config| config.unset(setting)).await {
            #[cfg(feature = "mqtt")]
            Ok(()) if matches!(setting, "mqtt.1" | "mqtt.2" | "mqtt.3") => {
                context.extension().request_mqtt_restart();
                String::from("OK - MQTT server reset to defaults; MQTT restarting")
            }
            Ok(()) if setting.starts_with("wifi.") => {
                String::from("OK - reset to default; reboot to apply")
            }
            Ok(()) => String::from("OK - reset to default"),
            Err(ConfigUpdateError::Invalid(ConfigError::UnknownSetting)) => {
                format!("Unknown config: {}", setting)
            }
            Err(error) => format!("Error: {}", error),
        },
    )
}

fn ota_status_text() -> String {
    let status = crate::platform::ota_status();
    if !status.available {
        return String::from("OTA: unavailable");
    }

    let next = status.next.unwrap_or("unknown");
    format!(
        "OTA: selected={} next={} next_size={}",
        status.selected, next, status.next_size
    )
}
