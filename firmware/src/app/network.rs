//! Network settings, stored in app.conf alongside the repeater settings.
use alloc::string::String;
use core::fmt::Write;

use mcrs_repeater::config::{
    ConfigError, ConfigSource, ExtensionConfig, bool_text, parse_bool, write_escaped_value,
};

#[cfg(feature = "mqtt")]
pub use crate::mqtt::MqttConfig;

const MAX_WIFI_SSID_LEN: usize = 32;
const MIN_WIFI_PASSWORD_LEN: usize = 8;
const MAX_WIFI_PASSWORD_LEN: usize = 63;
#[cfg(feature = "mqtt")]
const MAX_MQTT_VALUE_LEN: usize = 255;
#[cfg(feature = "mqtt")]
pub const MQTT_SERVER_COUNT: usize = 3;

pub const INVALID_NTP_HOST: ConfigError = ConfigError::Extension("invalid NTP host");
pub const INVALID_WIFI_CONFIG: ConfigError = ConfigError::Extension("invalid Wi-Fi setting");
#[cfg(feature = "mqtt")]
pub const INVALID_MQTT_CONFIG: ConfigError = ConfigError::Extension("invalid MQTT setting");

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetworkConfig {
    ntp: String,
    wifi: WifiConfig,
    #[cfg(feature = "mqtt")]
    mqtt: [MqttConfig; MQTT_SERVER_COUNT],
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WifiConfig {
    ssid: String,
    password: String,
    telnet: bool,
}

impl WifiConfig {
    pub fn ssid(&self) -> &str {
        &self.ssid
    }
    pub fn password(&self) -> &str {
        &self.password
    }
    pub fn telnet(&self) -> bool {
        self.telnet
    }
}

impl NetworkConfig {
    pub fn ntp(&self) -> &str {
        &self.ntp
    }

    pub fn set_ntp(&mut self, value: &str) -> Result<(), ConfigError> {
        validate_ntp_host(value)?;
        self.ntp = value.into();
        Ok(())
    }

    pub fn wifi(&self) -> &WifiConfig {
        &self.wifi
    }

    pub fn set_wifi_ssid(&mut self, value: &str) -> Result<(), ConfigError> {
        if value.len() > MAX_WIFI_SSID_LEN {
            return Err(INVALID_WIFI_CONFIG);
        }
        self.wifi.ssid = value.into();
        Ok(())
    }

    pub fn set_wifi_password(&mut self, value: &str) -> Result<(), ConfigError> {
        if !(value.is_empty()
            || (MIN_WIFI_PASSWORD_LEN..=MAX_WIFI_PASSWORD_LEN).contains(&value.len()))
        {
            return Err(INVALID_WIFI_CONFIG);
        }
        self.wifi.password = value.into();
        Ok(())
    }

    pub fn set_wifi_telnet(&mut self, enabled: bool) {
        self.wifi.telnet = enabled;
    }

    #[cfg(feature = "mqtt")]
    pub fn mqtt(&self, index: usize) -> Option<&MqttConfig> {
        self.mqtt.get(index)
    }

    #[cfg(feature = "mqtt")]
    pub fn set_mqtt_value(
        &mut self,
        index: usize,
        key: &str,
        value: &str,
    ) -> Result<(), ConfigError> {
        if value.len() > MAX_MQTT_VALUE_LEN {
            return Err(INVALID_MQTT_CONFIG);
        }
        let mqtt = self.mqtt.get_mut(index).ok_or(INVALID_MQTT_CONFIG)?;
        match key {
            "host" => {
                if !value.is_empty()
                    && crate::mqtt::transport::Endpoint::parse(value, mqtt.port).is_err()
                {
                    return Err(INVALID_MQTT_CONFIG);
                }
                mqtt.host = value.into();
            }
            "port" => mqtt.port = value.parse().map_err(|_| INVALID_MQTT_CONFIG)?,
            "username" => mqtt.username = value.into(),
            "password" => mqtt.password = value.into(),
            "auth" => mqtt.auth = crate::mqtt::AuthMode::parse(value).ok_or(INVALID_MQTT_CONFIG)?,
            "auth.audience" | "audience" => mqtt.auth_audience = value.into(),
            "topic.root" => mqtt.topic_root = value.into(),
            "iata" => mqtt.iata = value.into(),
            _ => return Err(INVALID_MQTT_CONFIG),
        }
        Ok(())
    }

    #[cfg(feature = "mqtt")]
    fn decode_mqtt(&mut self, key: &str, value: String, source: ConfigSource) -> Result<(), ()> {
        let (server, field) = parse_mqtt_key(key).ok_or(())?;
        let mqtt = self.mqtt.get_mut(server).ok_or(())?;
        match field {
            "host" => mqtt.host = value,
            "port" => mqtt.port = value.parse().map_err(|_| ())?,
            "username" => mqtt.username = value,
            "password" => mqtt.password = value,
            "auth" => mqtt.auth = crate::mqtt::AuthMode::parse(&value).ok_or(())?,
            "auth.audience" | "audience" => mqtt.auth_audience = value,
            "topic.root" => mqtt.topic_root = value,
            "iata" => mqtt.iata = value,
            _ if source.is_strict() => return Err(()),
            _ => {}
        }
        Ok(())
    }
}

impl ExtensionConfig for NetworkConfig {
    fn decode(
        &mut self,
        key: &str,
        value: String,
        source: ConfigSource,
    ) -> Option<Result<(), ()>> {
        match key {
            "ntp" => {
                if validate_ntp_host(&value).is_err() {
                    return Some(Err(()));
                }
                self.ntp = value;
            }
            "wifi.ssid" => self.wifi.ssid = value,
            "wifi.pass" => self.wifi.password = value,
            "wifi.telnet" => match parse_bool(&value) {
                Some(enabled) => self.wifi.telnet = enabled,
                None => return Some(Err(())),
            },
            // The base file contains optional MQTT settings for all builds.
            #[cfg(not(feature = "mqtt"))]
            key if key.starts_with("mqtt.") && source == ConfigSource::Defaults => {}
            #[cfg(feature = "mqtt")]
            key if key.starts_with("mqtt.") => return Some(self.decode_mqtt(key, value, source)),
            _ => return None,
        }
        Some(Ok(()))
    }

    fn validate(&self) -> bool {
        if validate_wifi_config(&self.wifi).is_err() {
            return false;
        }
        #[cfg(feature = "mqtt")]
        for mqtt in &self.mqtt {
            if !mqtt.host.is_empty()
                && crate::mqtt::transport::Endpoint::parse(&mqtt.host, mqtt.port).is_err()
            {
                return false;
            }
            if [
                &mqtt.host,
                &mqtt.username,
                &mqtt.password,
                &mqtt.auth_audience,
                &mqtt.topic_root,
                &mqtt.iata,
            ]
            .iter()
            .any(|value| value.len() > MAX_MQTT_VALUE_LEN)
            {
                return false;
            }
        }
        true
    }

    fn encode_changed(&self, defaults: &Self, redact_secrets: bool, out: &mut String) {
        if self.ntp != defaults.ntp {
            let _ = writeln!(out, "ntp={}", self.ntp);
        }
        if self.wifi.ssid != defaults.wifi.ssid {
            out.push_str("wifi.ssid=");
            write_escaped_value(out, &self.wifi.ssid);
            out.push('\n');
        }
        if self.wifi.password != defaults.wifi.password {
            out.push_str("wifi.pass=");
            if redact_secrets && !self.wifi.password.is_empty() {
                out.push_str("<redacted>");
            } else {
                write_escaped_value(out, &self.wifi.password);
            }
            out.push('\n');
        }
        if self.wifi.telnet != defaults.wifi.telnet {
            let _ = writeln!(out, "wifi.telnet={}", bool_text(self.wifi.telnet));
        }
        #[cfg(feature = "mqtt")]
        for (index, mqtt) in self.mqtt.iter().enumerate() {
            write_mqtt_config(out, index, mqtt, Some(&defaults.mqtt[index]), redact_secrets);
        }
    }

    fn encode_all(&self, redact_secrets: bool, out: &mut String) {
        let _ = writeln!(out, "ntp={}", self.ntp);
        out.push_str("wifi.ssid=");
        write_escaped_value(out, &self.wifi.ssid);
        out.push('\n');
        if redact_secrets && !self.wifi.password.is_empty() {
            out.push_str("wifi.pass=<redacted>\n");
        } else {
            out.push_str("wifi.pass=");
            write_escaped_value(out, &self.wifi.password);
            out.push('\n');
        }
        let _ = writeln!(out, "wifi.telnet={}", bool_text(self.wifi.telnet));
        #[cfg(feature = "mqtt")]
        for (index, mqtt) in self.mqtt.iter().enumerate() {
            write_mqtt_config(out, index, mqtt, None, redact_secrets);
        }
    }

    fn unset(&mut self, setting: &str, defaults: &Self) -> Result<(), ConfigError> {
        #[cfg(feature = "mqtt")]
        if let Some(number) = setting.strip_prefix("mqtt.")
            && !number.contains('.')
        {
            let index = number
                .parse::<usize>()
                .ok()
                .and_then(|number| number.checked_sub(1))
                .filter(|index| *index < MQTT_SERVER_COUNT)
                .ok_or(ConfigError::UnknownSetting)?;
            self.mqtt[index] = defaults.mqtt[index].clone();
            return Ok(());
        }
        match setting {
            "ntp" => self.ntp = defaults.ntp.clone(),
            "wifi.ssid" => self.wifi.ssid = defaults.wifi.ssid.clone(),
            "wifi.pass" => self.wifi.password = defaults.wifi.password.clone(),
            "wifi.telnet" => self.wifi.telnet = defaults.wifi.telnet,
            _ => return Err(ConfigError::UnknownSetting),
        }
        Ok(())
    }
}

/// Splits `mqtt.<number>.<field>` into a server index and field name.
#[cfg(feature = "mqtt")]
pub fn parse_mqtt_key(key: &str) -> Option<(usize, &str)> {
    let key = key.strip_prefix("mqtt.")?;
    let (number, field) = key.split_once('.')?;
    let index = number.parse::<usize>().ok()?.checked_sub(1)?;
    (index < MQTT_SERVER_COUNT).then_some((index, field))
}

fn validate_ntp_host(host: &str) -> Result<(), ConfigError> {
    if host.is_empty()
        || host.len() > 253
        || !host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return Err(INVALID_NTP_HOST);
    }
    Ok(())
}

fn validate_wifi_config(wifi: &WifiConfig) -> Result<(), ConfigError> {
    if wifi.ssid.len() > MAX_WIFI_SSID_LEN
        || !(wifi.password.is_empty()
            || (MIN_WIFI_PASSWORD_LEN..=MAX_WIFI_PASSWORD_LEN).contains(&wifi.password.len()))
    {
        return Err(INVALID_WIFI_CONFIG);
    }
    Ok(())
}

#[cfg(feature = "mqtt")]
fn write_mqtt_config(
    out: &mut String,
    index: usize,
    mqtt: &MqttConfig,
    defaults: Option<&MqttConfig>,
    redact: bool,
) {
    let prefix = index + 1;
    let values = [
        (
            "host",
            mqtt.host.as_str(),
            defaults.map(|d| d.host.as_str()),
            false,
        ),
        (
            "username",
            mqtt.username.as_str(),
            defaults.map(|d| d.username.as_str()),
            false,
        ),
        (
            "password",
            mqtt.password.as_str(),
            defaults.map(|d| d.password.as_str()),
            true,
        ),
        (
            "auth",
            mqtt.auth.as_str(),
            defaults.map(|d| d.auth.as_str()),
            false,
        ),
        (
            "auth.audience",
            mqtt.auth_audience.as_str(),
            defaults.map(|d| d.auth_audience.as_str()),
            false,
        ),
        (
            "topic.root",
            mqtt.topic_root.as_str(),
            defaults.map(|d| d.topic_root.as_str()),
            false,
        ),
        (
            "iata",
            mqtt.iata.as_str(),
            defaults.map(|d| d.iata.as_str()),
            false,
        ),
    ];
    for (key, value, default, secret) in values {
        if default == Some(value) {
            continue;
        }
        let _ = write!(out, "mqtt.{prefix}.{key}=");
        if redact && secret && !value.is_empty() {
            out.push_str("<redacted>");
        } else {
            write_escaped_value(out, value);
        }
        out.push('\n');
    }
    if defaults.is_none_or(|default| default.port != mqtt.port) {
        let _ = writeln!(out, "mqtt.{prefix}.port={}", mqtt.port);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{format, vec::Vec};
    use mcrs_repeater::{
        config::AppConfig,
        platform::storage::{Error, Storage},
    };

    struct MemoryStorage(Option<Vec<u8>>);

    impl Storage for MemoryStorage {
        fn read(&mut self, _key: &str, buffer: &mut [u8]) -> Result<usize, Error> {
            let data = self.0.as_ref().ok_or(Error::NotFound)?;
            buffer
                .get_mut(..data.len())
                .ok_or(Error::BufferTooSmall)?
                .copy_from_slice(data);
            Ok(data.len())
        }

        fn write_atomic(&mut self, _key: &str, data: &[u8]) -> Result<(), Error> {
            self.0 = Some(data.to_vec());
            Ok(())
        }
    }

    fn defaults() -> AppConfig<NetworkConfig> {
        AppConfig::generated_defaults([7; 32])
    }

    fn saved(config: &AppConfig<NetworkConfig>) -> String {
        let mut storage = MemoryStorage(None);
        config.save(&mut storage).unwrap();
        String::from_utf8(storage.0.unwrap()).unwrap()
    }

    fn reload(config: &AppConfig<NetworkConfig>) -> AppConfig<NetworkConfig> {
        let mut storage = MemoryStorage(None);
        config.save(&mut storage).unwrap();
        AppConfig::load_or_create(&mut storage, defaults())
    }

    #[test]
    fn base_defaults_and_selected_profile_are_valid() {
        // Generating defaults panics if the base file or the profile is invalid.
        assert!(validate_ntp_host(defaults().extension().ntp()).is_ok());
    }

    #[test]
    fn ntp_override_round_trips_and_resets() {
        let mut config = defaults();
        let default_ntp = String::from(config.extension().ntp());
        for host in ["time.example.net", "192.0.2.1"] {
            config.extension_mut().set_ntp(host).unwrap();
            assert_eq!(reload(&config).extension().ntp(), host);
            assert!(
                config
                    .export_config(true)
                    .contains(&format!("ntp={host}\n"))
            );
        }
        for host in [
            "",
            "time.example.net:123",
            "https://time.example.net",
            "a b",
            "a\nb",
        ] {
            assert_eq!(config.extension_mut().set_ntp(host), Err(INVALID_NTP_HOST));
        }
        config.unset("ntp").unwrap();
        assert_eq!(config.extension().ntp(), default_ntp);
        assert!(!saved(&config).contains("ntp="));
    }

    #[test]
    fn wifi_config_round_trips_escaped_values() {
        let mut config = defaults();
        config
            .extension_mut()
            .set_wifi_ssid("lab\\network\nwest")
            .unwrap();
        config
            .extension_mut()
            .set_wifi_password("password\\with\\slashes")
            .unwrap();
        assert_eq!(
            reload(&config).extension().wifi(),
            config.extension().wifi()
        );
    }

    #[test]
    fn wifi_password_is_redacted_from_effective_and_delta_config() {
        let mut config = defaults();
        config.extension_mut().set_wifi_ssid("lab").unwrap();
        config
            .extension_mut()
            .set_wifi_password("supersecret")
            .unwrap();
        let mut effective = String::new();
        config.write_effective_config(&mut effective);
        for rendered in [
            effective,
            config.export_config(true),
            config.export_config_delta(),
        ] {
            assert!(rendered.contains("wifi.pass=<redacted>\n"));
            assert!(!rendered.contains("supersecret"));
        }
        assert!(
            config
                .export_config(false)
                .contains("wifi.pass=supersecret\n")
        );
        assert!(saved(&config).contains("wifi.pass=supersecret\n"));
    }

    #[test]
    fn empty_wifi_password_round_trips() {
        let mut config = defaults();
        config.extension_mut().set_wifi_ssid("open-network").unwrap();
        config.extension_mut().set_wifi_password("").unwrap();
        let reloaded = reload(&config);
        assert_eq!(reloaded.extension().wifi().ssid(), "open-network");
        assert_eq!(reloaded.extension().wifi().password(), "");
    }

    #[test]
    fn wifi_telnet_round_trips_and_unset_removes_override() {
        let mut config = defaults();
        let default_telnet = config.extension().wifi().telnet();
        config.extension_mut().set_wifi_telnet(!default_telnet);
        assert_eq!(
            reload(&config).extension().wifi().telnet(),
            !default_telnet
        );
        assert!(saved(&config).contains("wifi.telnet="));
        config.unset("wifi.telnet").expect("known setting");
        assert_eq!(config.extension().wifi().telnet(), default_telnet);
        assert!(!saved(&config).contains("wifi.telnet="));
    }

    #[test]
    fn wifi_lengths_are_validated() {
        let mut network = NetworkConfig::default();
        assert!(network.set_wifi_ssid(&"x".repeat(MAX_WIFI_SSID_LEN)).is_ok());
        assert_eq!(
            network.set_wifi_ssid(&"x".repeat(MAX_WIFI_SSID_LEN + 1)),
            Err(INVALID_WIFI_CONFIG)
        );
        assert!(network.set_wifi_password("").is_ok());
        assert!(network.set_wifi_password("12345678").is_ok());
        assert!(network.set_wifi_password("short").is_err());
        assert!(
            network
                .set_wifi_password(&"x".repeat(MAX_WIFI_PASSWORD_LEN + 1))
                .is_err()
        );
    }

    #[test]
    fn decoding_rejects_unknown_profile_settings_and_invalid_values() {
        let mut network = NetworkConfig::default();
        assert_eq!(
            network.decode("wifi.typo", String::new(), ConfigSource::Profile),
            None
        );
        assert_eq!(
            network.decode("name", String::new(), ConfigSource::Profile),
            None
        );
        assert_eq!(
            network.decode("wifi.telnet", "maybe".into(), ConfigSource::Storage),
            Some(Err(()))
        );
        assert_eq!(
            network.decode("ntp", "bad host".into(), ConfigSource::Defaults),
            Some(Err(()))
        );
        network.wifi.ssid = "x".repeat(MAX_WIFI_SSID_LEN + 1);
        assert!(!network.validate());
    }

    #[cfg(not(feature = "mqtt"))]
    #[test]
    fn mqtt_defaults_are_ignored_without_mqtt_support() {
        let mut network = NetworkConfig::default();
        assert_eq!(
            network.decode("mqtt.1.host", String::new(), ConfigSource::Defaults),
            Some(Ok(()))
        );
        assert_eq!(
            network.decode("mqtt.1.host", String::new(), ConfigSource::Profile),
            None
        );
    }

    #[cfg(feature = "mqtt")]
    #[test]
    fn delta_export_redacts_only_changed_mqtt_passwords() {
        let mut config = defaults();
        config
            .extension_mut()
            .set_mqtt_value(0, "password", "device-secret")
            .unwrap();
        let delta = config.export_config_delta();
        assert!(delta.contains("mqtt.1.password=<redacted>\n"));
        assert!(!delta.contains("device-secret"));
        assert!(!delta.contains("mqtt.2.password="));
        assert!(saved(&config).contains("mqtt.1.password=device-secret\n"));
    }

    #[cfg(feature = "mqtt")]
    #[test]
    fn mqtt_settings_round_trip() {
        let mut config = defaults();
        config
            .extension_mut()
            .set_mqtt_value(1, "host", "mqtts://broker.example")
            .unwrap();
        config
            .extension_mut()
            .set_mqtt_value(1, "auth", "device")
            .unwrap();
        assert_eq!(
            reload(&config).extension().mqtt(1),
            config.extension().mqtt(1)
        );
        assert_eq!(
            config.extension_mut().set_mqtt_value(1, "typo", "x"),
            Err(INVALID_MQTT_CONFIG)
        );
    }

    #[cfg(feature = "mqtt")]
    #[test]
    fn unset_mqtt_server_restores_all_defaults() {
        let mut config = AppConfig::<NetworkConfig>::generated_defaults([11; 32]);
        let network = config.extension_mut();
        network.set_mqtt_value(0, "host", "broker.example").unwrap();
        network.set_mqtt_value(0, "port", "2883").unwrap();
        network.set_mqtt_value(0, "password", "secret").unwrap();

        config.unset("mqtt.1").expect("known MQTT server");

        assert_eq!(
            config.extension().mqtt(0),
            AppConfig::<NetworkConfig>::generated_defaults([11; 32])
                .extension()
                .mqtt(0)
        );
        assert!(config.unset("mqtt.0").is_err());
        assert!(config.unset("mqtt.4").is_err());
    }
}
