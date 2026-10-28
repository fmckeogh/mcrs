use alloc::{string::String, vec::Vec};
use core::fmt::{self, Write};

use super::{
    LoopDetection,
    identity::{self, Identity, PrivateKey},
    regions::{RegionError, RegionMap},
};

// Unit tests use the base defaults alone, whichever profile is selected.
#[cfg(all(mcrs_profile, not(test)))]
const PROFILE_CONFIG: &str = include_str!(env!("MCRS_PROFILE_PATH"));
#[cfg(any(not(mcrs_profile), test))]
const PROFILE_CONFIG: &str = "";

fn clamp_advert_interval(minutes: u32) -> u32 {
    match minutes {
        1..=59 => 60,
        _ => minutes,
    }
}

const APP_CONFIG_KEY: &str = "app.conf";
const APP_CONFIG_TEXT_VERSION: u8 = 1;
const APP_CONFIG_MAX_LEN: usize = 4096;
const MAX_REMOTE_PASSWORD_LEN: usize = 64;
const MAX_NODE_NAME_LEN: usize = 31;
const MAX_OWNER_INFO_LEN: usize = 119;
const UNPROVISIONED_CONFIG_TEXT: &[u8] = b"# MCRS app.conf\nversion=1\n";
const COORDINATE_SCALE: i32 = 1_000_000;
const MIN_LATITUDE_MICRODEGREES: i32 = -90 * COORDINATE_SCALE;
const MAX_LATITUDE_MICRODEGREES: i32 = 90 * COORDINATE_SCALE;
const MIN_LONGITUDE_MICRODEGREES: i32 = -180 * COORDINATE_SCALE;
const MAX_LONGITUDE_MICRODEGREES: i32 = 180 * COORDINATE_SCALE;

/// Settings for firmware features outside the repeater, such as networking.
/// They are layered and stored in app.conf alongside the repeater settings.
pub trait ExtensionConfig: Clone + Default {
    /// Applies one setting from a config layer. Returns `None` for keys the
    /// extension does not own, and `Some(Err(()))` for invalid values.
    fn decode(&mut self, key: &str, value: String, source: ConfigSource)
    -> Option<Result<(), ()>>;

    /// Checks the combined settings after a layer has been applied.
    fn validate(&self) -> bool {
        true
    }

    /// Writes the settings that differ from `defaults`.
    fn encode_changed(&self, defaults: &Self, redact_secrets: bool, out: &mut String);

    /// Writes every setting.
    fn encode_all(&self, redact_secrets: bool, out: &mut String);

    /// Restores one setting to its default, or returns `UnknownSetting`.
    fn unset(&mut self, setting: &str, defaults: &Self) -> Result<(), ConfigError>;
}

impl ExtensionConfig for () {
    fn decode(
        &mut self,
        _key: &str,
        _value: String,
        source: ConfigSource,
    ) -> Option<Result<(), ()>> {
        // The base file contains settings for optional firmware features.
        (source == ConfigSource::Defaults).then_some(Ok(()))
    }

    fn encode_changed(&self, _defaults: &Self, _redact_secrets: bool, _out: &mut String) {}

    fn encode_all(&self, _redact_secrets: bool, _out: &mut String) {}

    fn unset(&mut self, _setting: &str, _defaults: &Self) -> Result<(), ConfigError> {
        Err(ConfigError::UnknownSetting)
    }
}

pub struct AppConfig<C> {
    identity: Identity,
    private_key: PrivateKey,
    identity_label: &'static str,
    latitude_microdegrees: Option<i32>,
    longitude_microdegrees: Option<i32>,
    node_name: String,
    owner_info: String,
    remote_cli_password: String,
    acl: super::acl::Acl,
    radio: RadioConfig,
    regions: RegionMap,
    region_capture: bool,
    repeat_enabled: bool,
    flood_max_unscoped_hops: u8,
    flood_max_advert_hops: u8,
    advert_interval_minutes: u32,
    flood_advert_interval_hours: u32,
    loop_detection: LoopDetection,
    path_hash_mode: u8,
    duty_cycle_percent: u8,
    extension: C,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RadioConfig {
    pub receive_frequency_hz: u32,
    pub spreading_factor: u8,
    pub bandwidth_hz: u32,
    pub coding_rate_denominator: u8,
    pub transmit_power_dbm: i32,
}

impl<C: ExtensionConfig> AppConfig<C> {
    pub fn generated_defaults(identity_seed: [u8; 32]) -> Self {
        Self::from_stored(
            StoredAppConfig::default_with_identity_seed(identity_seed),
            "generated",
        )
    }

    pub fn load_or_create<S>(storage: &mut S, defaults: Self) -> Self
    where
        S: crate::platform::storage::Storage,
    {
        let default_identity_label = defaults.identity_label;
        let default_stored = StoredAppConfig::from_app_config(&defaults);

        let (stored, identity_label) = match load_config_file(storage, &default_stored) {
            Some((stored, needs_rewrite)) => {
                if needs_rewrite {
                    let _ = write_config_file(storage, &stored);
                }
                (stored, "storage")
            }
            None => {
                let _ = write_config_file(storage, &default_stored);
                (default_stored, default_identity_label)
            }
        };

        Self::from_stored(stored, identity_label)
    }

    fn from_stored(stored: StoredAppConfig<C>, identity_label: &'static str) -> Self {
        Self {
            identity: Identity::from_private_key(stored.private_key),
            private_key: stored.private_key,
            identity_label,
            latitude_microdegrees: stored.latitude_microdegrees,
            longitude_microdegrees: stored.longitude_microdegrees,
            node_name: stored.node_name,
            owner_info: stored.owner_info,
            remote_cli_password: stored.remote_cli_password,
            acl: stored.acl,
            radio: stored.radio,
            regions: stored.regions,
            region_capture: stored.region_capture,
            repeat_enabled: stored.repeat_enabled,
            flood_max_unscoped_hops: stored.flood_max_unscoped_hops,
            flood_max_advert_hops: stored.flood_max_advert_hops,
            advert_interval_minutes: stored.advert_interval_minutes,
            flood_advert_interval_hours: stored.flood_advert_interval_hours,
            loop_detection: stored.loop_detection,
            path_hash_mode: stored.path_hash_mode,
            duty_cycle_percent: stored.duty_cycle_percent,
            extension: stored.extension,
        }
    }

    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    pub fn identity_label(&self) -> &'static str {
        self.identity_label
    }

    pub fn private_key(&self) -> &PrivateKey {
        &self.private_key
    }

    pub fn latitude_microdegrees(&self) -> Option<i32> {
        self.latitude_microdegrees
    }

    pub fn longitude_microdegrees(&self) -> Option<i32> {
        self.longitude_microdegrees
    }

    pub fn location(&self) -> Option<(i32, i32)> {
        Some((self.latitude_microdegrees?, self.longitude_microdegrees?))
    }

    pub fn node_name(&self) -> &str {
        &self.node_name
    }

    pub fn owner_info(&self) -> &str {
        &self.owner_info
    }

    pub fn set_owner_info(&mut self, value: &str) {
        self.owner_info = fit_owner_info(value);
    }

    pub fn remote_cli_password(&self) -> &str {
        &self.remote_cli_password
    }

    pub fn extension(&self) -> &C {
        &self.extension
    }

    pub fn extension_mut(&mut self) -> &mut C {
        &mut self.extension
    }

    pub fn acl(&self) -> &super::acl::Acl {
        &self.acl
    }

    pub fn set_acl(&mut self, key: &str, role: &str) -> Result<(), ConfigError> {
        let previous = self.acl.clone();
        self.acl
            .set(key, role)
            .map_err(|_| ConfigError::InvalidAcl)?;
        if encode_config_text(&StoredAppConfig::from_app_config(self)).len() > APP_CONFIG_MAX_LEN {
            self.acl = previous;
            return Err(ConfigError::InvalidAcl);
        }
        Ok(())
    }

    pub fn unset(&mut self, setting: &str) -> Result<(), ConfigError> {
        if let Some(key) = setting.strip_prefix("acl ") {
            return self.set_acl(key.trim(), "unset");
        }
        let defaults = Self::from_stored(
            StoredAppConfig::default_with_private_key(self.private_key),
            "generated",
        );
        match setting {
            "name" => self.node_name = defaults.node_name,
            "owner.info" => self.owner_info = defaults.owner_info,
            "password" => self.remote_cli_password = defaults.remote_cli_password,
            "lat" => self.latitude_microdegrees = defaults.latitude_microdegrees,
            "lon" => self.longitude_microdegrees = defaults.longitude_microdegrees,
            "radio" => self.radio = defaults.radio,
            "freq" => self.radio.receive_frequency_hz = defaults.radio.receive_frequency_hz,
            "tx" => self.radio.transmit_power_dbm = defaults.radio.transmit_power_dbm,
            "dutycycle" => self.duty_cycle_percent = defaults.duty_cycle_percent,
            "flood.max.unscoped" => self.flood_max_unscoped_hops = defaults.flood_max_unscoped_hops,
            "flood.max.advert" => self.flood_max_advert_hops = defaults.flood_max_advert_hops,
            "repeat" => self.repeat_enabled = defaults.repeat_enabled,
            "advert.interval" => self.advert_interval_minutes = defaults.advert_interval_minutes,
            "flood.advert.interval" => {
                self.flood_advert_interval_hours = defaults.flood_advert_interval_hours
            }
            "loop.detect" => self.loop_detection = defaults.loop_detection,
            "path.hash.mode" => self.path_hash_mode = defaults.path_hash_mode,
            _ => return self.extension.unset(setting, &defaults.extension),
        }
        Ok(())
    }

    pub fn radio(&self) -> RadioConfig {
        self.radio
    }

    pub fn regions(&self) -> &RegionMap {
        &self.regions
    }

    pub fn repeat_enabled(&self) -> bool {
        self.repeat_enabled
    }

    pub fn set_repeat_enabled(&mut self, enabled: bool) {
        self.repeat_enabled = enabled;
    }

    pub fn region_capture(&self) -> bool {
        self.region_capture
    }

    pub fn flood_max_unscoped_hops(&self) -> u8 {
        self.flood_max_unscoped_hops
    }

    pub fn flood_max_advert_hops(&self) -> u8 {
        self.flood_max_advert_hops
    }

    pub fn advert_interval_minutes(&self) -> u32 {
        self.advert_interval_minutes
    }

    pub fn set_advert_interval_minutes(&mut self, minutes: u32) {
        self.advert_interval_minutes = clamp_advert_interval(minutes);
    }

    pub fn flood_advert_interval_hours(&self) -> u32 {
        self.flood_advert_interval_hours
    }

    pub fn set_flood_advert_interval_hours(&mut self, hours: u32) {
        self.flood_advert_interval_hours = hours;
    }

    pub fn path_hash_mode(&self) -> u8 {
        self.path_hash_mode
    }

    pub fn loop_detection(&self) -> LoopDetection {
        self.loop_detection
    }

    pub fn set_loop_detection(&mut self, mode: LoopDetection) {
        self.loop_detection = mode;
    }

    pub fn duty_cycle_percent(&self) -> u8 {
        self.duty_cycle_percent
    }

    pub fn set_node_name(&mut self, node_name: &str) -> Result<(), ConfigError> {
        self.node_name = fit_node_name(node_name)?;
        Ok(())
    }

    pub fn set_remote_cli_password(&mut self, password: &str) {
        self.remote_cli_password = fit_password(password);
    }

    pub fn set_private_key(&mut self, private_key: PrivateKey) {
        self.private_key = private_key;
    }

    pub fn set_latitude_microdegrees(
        &mut self,
        latitude_microdegrees: Option<i32>,
    ) -> Result<(), ConfigError> {
        if let Some(latitude) = latitude_microdegrees {
            validate_latitude(latitude)?;
        }
        self.latitude_microdegrees = latitude_microdegrees;
        Ok(())
    }

    pub fn set_longitude_microdegrees(
        &mut self,
        longitude_microdegrees: Option<i32>,
    ) -> Result<(), ConfigError> {
        if let Some(longitude) = longitude_microdegrees {
            validate_longitude(longitude)?;
        }
        self.longitude_microdegrees = longitude_microdegrees;
        Ok(())
    }

    pub fn set_radio(&mut self, radio: RadioConfig) -> Result<(), ConfigError> {
        radio.validate()?;
        self.radio = radio;
        Ok(())
    }

    pub fn set_transmit_power_dbm(&mut self, transmit_power_dbm: i32) -> Result<(), ConfigError> {
        self.radio.transmit_power_dbm = transmit_power_dbm;
        self.radio.validate()
    }

    pub fn set_duty_cycle_percent(&mut self, percent: u8) -> Result<(), ConfigError> {
        if !(1..=100).contains(&percent) {
            return Err(ConfigError::InvalidDutyCycle);
        }
        self.duty_cycle_percent = percent;
        Ok(())
    }

    pub fn put_region(&mut self, name: &str) -> Result<(), ConfigError> {
        Ok(self.regions.put_region(name)?)
    }

    pub fn remove_region(&mut self, name: &str) -> Result<(), ConfigError> {
        let defaults = StoredAppConfig::<C>::default_with_private_key(self.private_key);
        if let Some(region) = defaults.regions.find_by_name_prefix(name)
            && !region.is_wildcard()
            && region.display_name() == name.strip_prefix('#').unwrap_or(name)
        {
            return Err(ConfigError::InheritedRegion);
        }
        Ok(self.regions.remove_region(name)?)
    }

    pub fn set_region_flood_allowed(
        &mut self,
        name: &str,
        allowed: bool,
    ) -> Result<(), ConfigError> {
        Ok(self.regions.set_flood_allowed(name, allowed)?)
    }

    pub fn set_default_region(&mut self, name: Option<&str>) -> Result<(), ConfigError> {
        if let Some(name) = name
            && name != "*"
            && self.regions.find_by_name_prefix(name).is_none()
        {
            self.regions.put_region(name)?;
        }
        Ok(self.regions.set_default(name)?)
    }

    pub fn set_region_capture(&mut self, enabled: bool) -> Result<(), ConfigError> {
        self.region_capture = enabled;
        Ok(())
    }

    pub fn set_flood_max_unscoped_hops(&mut self, hops: u8) -> Result<(), ConfigError> {
        validate_flood_max_hops(hops)?;
        self.flood_max_unscoped_hops = hops;
        Ok(())
    }

    pub fn set_flood_max_advert_hops(&mut self, hops: u8) -> Result<(), ConfigError> {
        validate_flood_max_hops(hops)?;
        self.flood_max_advert_hops = hops;
        Ok(())
    }

    pub fn set_path_hash_mode(&mut self, mode: u8) -> Result<(), ConfigError> {
        validate_path_hash_mode(mode)?;
        self.path_hash_mode = mode;
        Ok(())
    }

    pub fn export_config(&self, redact_secrets: bool) -> String {
        String::from_utf8(encode_full_config_text_redacted(
            &StoredAppConfig::from_app_config(self),
            redact_secrets,
        ))
        .expect("UTF-8 config")
    }

    pub fn export_config_delta(&self) -> String {
        let config = StoredAppConfig::from_app_config(self);
        let defaults = StoredAppConfig::default_with_private_key(self.private_key);
        String::from_utf8(encode_sparse_config_text_redacted(&config, &defaults, true))
            .expect("UTF-8 config")
    }

    pub fn save<S>(&self, storage: &mut S) -> Result<(), crate::platform::storage::Error>
    where
        S: crate::platform::storage::Storage,
    {
        write_config_file(storage, &StoredAppConfig::from_app_config(self))
    }

    pub fn save_unprovisioned<S>(storage: &mut S) -> Result<(), crate::platform::storage::Error>
    where
        S: crate::platform::storage::Storage,
    {
        storage.write_atomic(APP_CONFIG_KEY, UNPROVISIONED_CONFIG_TEXT)
    }

    pub fn write_effective_config(&self, out: &mut String) {
        let rendered =
            encode_full_config_text_redacted(&StoredAppConfig::from_app_config(self), true);
        if let Ok(text) = core::str::from_utf8(&rendered) {
            out.push_str(text);
        }
    }
}

impl RadioConfig {
    pub fn packet_airtime_ms(self, payload_len: usize) -> u32 {
        let symbol_time_us = self.symbol_time_us();
        let low_data_rate_optimize = symbol_time_us >= 16_000;
        let payload_symbols = payload_symbol_count(
            payload_len,
            self.spreading_factor,
            self.coding_rate_denominator,
            low_data_rate_optimize,
        );
        let preamble_symbols_quarters = crate::radio::RECEIVE_PREAMBLE_LENGTH as u64 * 4 + 17;
        let preamble_us = div_ceil_u64(symbol_time_us * preamble_symbols_quarters, 4);
        let payload_us = payload_symbols as u64 * symbol_time_us;

        div_ceil_u64(preamble_us.saturating_add(payload_us), 1_000).min(u32::MAX as u64) as u32
    }

    pub fn symbol_time_ms(self) -> u32 {
        div_ceil_u64(self.symbol_time_us(), 1_000).min(u32::MAX as u64) as u32
    }

    fn symbol_time_us(self) -> u64 {
        let chips_per_symbol = 1u64 << self.spreading_factor.min(31);
        div_ceil_u64(
            chips_per_symbol * 1_000_000,
            self.bandwidth_hz.max(1) as u64,
        )
    }

    pub fn validate(self) -> Result<(), ConfigError> {
        if !(150_000_000..=2_500_000_000).contains(&self.receive_frequency_hz) {
            return Err(ConfigError::InvalidFrequency);
        }
        if !(7_000..=500_000).contains(&self.bandwidth_hz) {
            return Err(ConfigError::InvalidBandwidth);
        }
        if !(5..=12).contains(&self.spreading_factor) {
            return Err(ConfigError::InvalidSpreadingFactor);
        }
        if !(5..=8).contains(&self.coding_rate_denominator) {
            return Err(ConfigError::InvalidCodingRate);
        }
        if !(-9..=22).contains(&self.transmit_power_dbm) {
            return Err(ConfigError::InvalidTransmitPower);
        }
        Ok(())
    }
}

fn payload_symbol_count(
    payload_len: usize,
    spreading_factor: u8,
    coding_rate_denominator: u8,
    low_data_rate_optimize: bool,
) -> u32 {
    let sf = spreading_factor as i32;
    let crc_on = 1;
    let implicit_header = 0;
    let de = i32::from(low_data_rate_optimize);
    let numerator = 8 * payload_len as i32 - 4 * sf + 28 + 16 * crc_on - 20 * implicit_header;
    let denominator = 4 * (sf - 2 * de);
    let coded_symbols = if numerator <= 0 {
        0
    } else {
        ((numerator + denominator - 1) / denominator) as u32 * coding_rate_denominator as u32
    };

    8 + coded_symbols
}

fn div_ceil_u64(numerator: u64, denominator: u64) -> u64 {
    numerator / denominator + u64::from(!numerator.is_multiple_of(denominator))
}

fn generated_node_name(private_key: PrivateKey) -> String {
    let identity = Identity::from_private_key(private_key);
    let mut node_name = String::from("Repeater-");
    for byte in &identity.public_key()[..3] {
        push_hex_byte(&mut node_name, *byte);
    }
    node_name
}

#[derive(Clone)]
struct StoredAppConfig<C> {
    private_key: PrivateKey,
    latitude_microdegrees: Option<i32>,
    longitude_microdegrees: Option<i32>,
    node_name: String,
    owner_info: String,
    remote_cli_password: String,
    acl: super::acl::Acl,
    radio: RadioConfig,
    regions: RegionMap,
    region_capture: bool,
    repeat_enabled: bool,
    flood_max_unscoped_hops: u8,
    flood_max_advert_hops: u8,
    advert_interval_minutes: u32,
    flood_advert_interval_hours: u32,
    loop_detection: LoopDetection,
    path_hash_mode: u8,
    duty_cycle_percent: u8,
    extension: C,
}

impl<C: ExtensionConfig> StoredAppConfig<C> {
    fn default_with_identity_seed(identity_seed: [u8; 32]) -> Self {
        Self::default_with_private_key(PrivateKey::Seed(identity_seed))
    }

    fn default_with_private_key(private_key: PrivateKey) -> Self {
        Self::with_defaults_text(private_key, PROFILE_CONFIG)
            .expect("invalid embedded defaults or profile")
    }

    fn with_defaults_text(private_key: PrivateKey, overlay: &str) -> Option<Self> {
        // Only device-specific values are generated here. Fixed defaults live
        // in profiles/defaults.conf; the zero values below are parser initialization.
        let empty = Self {
            private_key,
            latitude_microdegrees: None,
            longitude_microdegrees: None,
            node_name: fit_node_name(&generated_node_name(private_key))
                .unwrap_or_else(|_| String::from("Repeater")),
            owner_info: String::new(),
            remote_cli_password: String::new(),
            acl: super::acl::Acl::default(),
            radio: RadioConfig {
                receive_frequency_hz: 0,
                spreading_factor: 0,
                bandwidth_hz: 0,
                coding_rate_denominator: 0,
                transmit_power_dbm: 0,
            },
            regions: RegionMap::new(),
            region_capture: false,
            repeat_enabled: false,
            flood_max_unscoped_hops: 0,
            flood_max_advert_hops: 0,
            advert_interval_minutes: 0,
            flood_advert_interval_hours: 0,
            loop_detection: LoopDetection::default(),
            path_hash_mode: 0,
            duty_cycle_percent: 0,
            extension: C::default(),
        };
        let defaults = decode_config_layer(
            include_bytes!("../../profiles/defaults.conf"),
            &empty,
            ConfigSource::Defaults,
        )?;
        decode_config_layer(overlay.as_bytes(), &defaults, ConfigSource::Profile)
    }

    fn from_app_config(config: &AppConfig<C>) -> Self {
        Self {
            private_key: config.private_key,
            latitude_microdegrees: config.latitude_microdegrees,
            longitude_microdegrees: config.longitude_microdegrees,
            node_name: fit_node_name(config.node_name())
                .unwrap_or_else(|_| String::from("Repeater")),
            owner_info: String::from(config.owner_info()),
            remote_cli_password: fit_password(config.remote_cli_password()),
            acl: config.acl.clone(),
            radio: config.radio,
            regions: config.regions.clone(),
            region_capture: config.region_capture,
            repeat_enabled: config.repeat_enabled,
            flood_max_unscoped_hops: config.flood_max_unscoped_hops,
            flood_max_advert_hops: config.flood_max_advert_hops,
            advert_interval_minutes: config.advert_interval_minutes,
            flood_advert_interval_hours: config.flood_advert_interval_hours,
            loop_detection: config.loop_detection,
            path_hash_mode: config.path_hash_mode,
            duty_cycle_percent: config.duty_cycle_percent,
            extension: config.extension.clone(),
        }
    }
}

fn load_config_file<S, C>(
    storage: &mut S,
    defaults: &StoredAppConfig<C>,
) -> Option<(StoredAppConfig<C>, bool)>
where
    S: crate::platform::storage::Storage,
    C: ExtensionConfig,
{
    let mut buffer = [0u8; APP_CONFIG_MAX_LEN];
    let len = storage.read(APP_CONFIG_KEY, &mut buffer).ok()?;
    let data = buffer.get(..len)?;

    let config = decode_config_text(data, defaults)?;
    let encoded = encode_config_text(&config);
    Some((config, data != encoded.as_slice()))
}

fn write_config_file<S, C>(
    storage: &mut S,
    config: &StoredAppConfig<C>,
) -> Result<(), crate::platform::storage::Error>
where
    S: crate::platform::storage::Storage,
    C: ExtensionConfig,
{
    let data = encode_config_text(config);
    if data.len() > APP_CONFIG_MAX_LEN {
        return Err(crate::platform::storage::Error::BufferTooSmall);
    }
    storage.write_atomic(APP_CONFIG_KEY, &data)
}

/// The layer a setting is read from: the base defaults, the selected build
/// profile, or the device's stored config.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigSource {
    Defaults,
    Profile,
    Storage,
}

impl ConfigSource {
    /// Unknown settings are errors in build-time layers, and ignored in storage.
    pub fn is_strict(self) -> bool {
        self != Self::Storage
    }
}

fn decode_config_text<C: ExtensionConfig>(
    data: &[u8],
    defaults: &StoredAppConfig<C>,
) -> Option<StoredAppConfig<C>> {
    decode_config_layer(data, defaults, ConfigSource::Storage)
}

fn decode_config_layer<C: ExtensionConfig>(
    data: &[u8],
    defaults: &StoredAppConfig<C>,
    source: ConfigSource,
) -> Option<StoredAppConfig<C>> {
    let strict = source.is_strict();
    if strict && data.len() > APP_CONFIG_MAX_LEN {
        return None;
    }
    let text = core::str::from_utf8(data).ok()?;
    let mut config = defaults.clone();
    let mut saw_node_name = false;

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let (key, value) = line.split_once('=')?;
        let key = key.trim();
        let value = unescape_value(value.trim()).ok()?;

        match key {
            "version" => {
                let version = value.parse::<u8>().ok()?;
                if strict && version != APP_CONFIG_TEXT_VERSION {
                    return None;
                }
            }
            "identity.seed" | "identity.expanded" => {
                if strict {
                    return None;
                }
                let private_key = PrivateKey::from_hex(&value)?;
                if private_key.config_key() != key {
                    return None;
                }
                config.private_key = private_key;
            }
            "identity.lat" => {
                config.latitude_microdegrees = parse_optional_coordinate(
                    &value,
                    MIN_LATITUDE_MICRODEGREES,
                    MAX_LATITUDE_MICRODEGREES,
                )?;
            }
            "identity.lon" => {
                config.longitude_microdegrees = parse_optional_coordinate(
                    &value,
                    MIN_LONGITUDE_MICRODEGREES,
                    MAX_LONGITUDE_MICRODEGREES,
                )?;
            }
            "owner.info" => config.owner_info = fit_owner_info(&value),
            "node.name" => {
                config.node_name = fit_node_name(&value).ok()?;
                saw_node_name = true;
            }
            "remote.password" => {
                config.remote_cli_password = fit_password(&value);
            }
            "radio.frequency_hz" => {
                config.radio.receive_frequency_hz = value.parse::<u32>().ok()?;
            }
            "radio.bandwidth_hz" => {
                config.radio.bandwidth_hz = value.parse::<u32>().ok()?;
            }
            "radio.spreading_factor" => {
                config.radio.spreading_factor = value.parse::<u8>().ok()?;
            }
            "radio.coding_rate" | "radio.coding_rate_denominator" => {
                config.radio.coding_rate_denominator = value.parse::<u8>().ok()?;
            }
            "radio.tx_power_dbm" => {
                config.radio.transmit_power_dbm = value.parse::<i32>().ok()?;
            }
            "radio.duty_cycle_percent" => {
                config.duty_cycle_percent = value.parse::<u8>().ok()?;
            }
            "region.default" => {
                config.regions.set_default_from_config(&value).ok()?;
            }
            "repeat" => {
                config.repeat_enabled = parse_repeat(&value)?;
            }
            "region.capture" => {
                config.region_capture = parse_bool(&value)?;
            }
            "flood.max.unscoped" => {
                config.flood_max_unscoped_hops = parse_flood_max_hops(&value)?;
            }
            "flood.max.advert" => {
                config.flood_max_advert_hops = parse_flood_max_hops(&value)?;
            }
            "advert.interval" => {
                config.advert_interval_minutes = clamp_advert_interval(value.parse::<u32>().ok()?);
            }
            "flood.advert.interval" => {
                config.flood_advert_interval_hours = value.parse::<u32>().ok()?;
            }
            "path.hash.mode" => {
                config.path_hash_mode = parse_path_hash_mode(&value)?;
            }
            "loop.detect" => {
                config.loop_detection = LoopDetection::parse(&value)?;
            }
            key if key.starts_with("acl.") => {
                config.acl.set(key.strip_prefix("acl.")?, &value).ok()?;
            }
            key if key.starts_with("region.") => {
                let name = key.strip_prefix("region.")?;
                let allowed = parse_bool(&value)?;
                config.regions.set_region_from_config(name, allowed).ok()?;
            }
            key => match config.extension.decode(key, value, source) {
                Some(Ok(())) => {}
                Some(Err(())) => return None,
                None if strict => return None,
                None => {}
            },
        }
    }

    if !saw_node_name
        && config.private_key != defaults.private_key
        && defaults.node_name == generated_node_name(defaults.private_key)
    {
        config.node_name = fit_node_name(&generated_node_name(config.private_key)).ok()?;
    }
    config.radio.validate().ok()?;
    if !(1..=100).contains(&config.duty_cycle_percent) {
        return None;
    }
    validate_flood_max_hops(config.flood_max_unscoped_hops).ok()?;
    validate_flood_max_hops(config.flood_max_advert_hops).ok()?;
    validate_path_hash_mode(config.path_hash_mode).ok()?;
    if !config.extension.validate() {
        return None;
    }
    Some(config)
}

fn encode_config_text<C: ExtensionConfig>(config: &StoredAppConfig<C>) -> Vec<u8> {
    let defaults = StoredAppConfig::default_with_private_key(config.private_key);
    encode_sparse_config_text(config, &defaults)
}

fn encode_sparse_config_text<C: ExtensionConfig>(
    config: &StoredAppConfig<C>,
    defaults: &StoredAppConfig<C>,
) -> Vec<u8> {
    encode_sparse_config_text_redacted(config, defaults, false)
}

fn encode_sparse_config_text_redacted<C: ExtensionConfig>(
    config: &StoredAppConfig<C>,
    defaults: &StoredAppConfig<C>,
    redact_secrets: bool,
) -> Vec<u8> {
    let mut out = String::new();
    let _ = writeln!(&mut out, "# MCRS app.conf");
    let _ = writeln!(&mut out, "version={}", APP_CONFIG_TEXT_VERSION);

    config.acl.write_overrides(&defaults.acl, &mut out);

    // Storage always carries the device identity. A delta export compares
    // settings against this device's defaults, so the identity is omitted.
    if !redact_secrets {
        out.push_str(config.private_key.config_key());
        out.push('=');
        for byte in config.private_key.as_bytes() {
            push_hex_byte(&mut out, *byte);
        }
        out.push('\n');
    }

    if config.latitude_microdegrees != defaults.latitude_microdegrees {
        out.push_str("identity.lat=");
        write_optional_coordinate(&mut out, config.latitude_microdegrees);
        out.push('\n');
    }

    if config.longitude_microdegrees != defaults.longitude_microdegrees {
        out.push_str("identity.lon=");
        write_optional_coordinate(&mut out, config.longitude_microdegrees);
        out.push('\n');
    }

    if config.node_name != defaults.node_name {
        out.push_str("node.name=");
        write_escaped_value(&mut out, &config.node_name);
        out.push('\n');
    }

    if config.owner_info != defaults.owner_info {
        out.push_str("owner.info=");
        write_escaped_value(&mut out, &config.owner_info);
        out.push('\n');
    }

    if config.remote_cli_password != defaults.remote_cli_password {
        out.push_str("remote.password=");
        if redact_secrets {
            out.push_str("<redacted>");
        } else {
            write_escaped_value(&mut out, &config.remote_cli_password);
        }
        out.push('\n');
    }

    if config.radio.receive_frequency_hz != defaults.radio.receive_frequency_hz {
        let _ = writeln!(
            &mut out,
            "radio.frequency_hz={}",
            config.radio.receive_frequency_hz
        );
    }
    if config.radio.bandwidth_hz != defaults.radio.bandwidth_hz {
        let _ = writeln!(&mut out, "radio.bandwidth_hz={}", config.radio.bandwidth_hz);
    }
    if config.radio.spreading_factor != defaults.radio.spreading_factor {
        let _ = writeln!(
            &mut out,
            "radio.spreading_factor={}",
            config.radio.spreading_factor
        );
    }
    if config.radio.coding_rate_denominator != defaults.radio.coding_rate_denominator {
        let _ = writeln!(
            &mut out,
            "radio.coding_rate={}",
            config.radio.coding_rate_denominator
        );
    }
    if config.radio.transmit_power_dbm != defaults.radio.transmit_power_dbm {
        let _ = writeln!(
            &mut out,
            "radio.tx_power_dbm={}",
            config.radio.transmit_power_dbm
        );
    }
    if config.duty_cycle_percent != defaults.duty_cycle_percent {
        let _ = writeln!(
            &mut out,
            "radio.duty_cycle_percent={}",
            config.duty_cycle_percent
        );
    }
    config
        .regions
        .write_config_lines_changed(&defaults.regions, &mut out);
    if config.repeat_enabled != defaults.repeat_enabled {
        let _ = writeln!(&mut out, "repeat={}", repeat_text(config.repeat_enabled));
    }
    if config.region_capture != defaults.region_capture {
        let _ = writeln!(
            &mut out,
            "region.capture={}",
            bool_text(config.region_capture)
        );
    }
    if config.flood_max_unscoped_hops != defaults.flood_max_unscoped_hops {
        let _ = writeln!(
            &mut out,
            "flood.max.unscoped={}",
            config.flood_max_unscoped_hops
        );
    }
    if config.flood_max_advert_hops != defaults.flood_max_advert_hops {
        let _ = writeln!(
            &mut out,
            "flood.max.advert={}",
            config.flood_max_advert_hops
        );
    }
    if config.advert_interval_minutes != defaults.advert_interval_minutes {
        let _ = writeln!(
            &mut out,
            "advert.interval={}",
            config.advert_interval_minutes
        );
    }
    if config.flood_advert_interval_hours != defaults.flood_advert_interval_hours {
        let _ = writeln!(
            &mut out,
            "flood.advert.interval={}",
            config.flood_advert_interval_hours
        );
    }
    if config.path_hash_mode != defaults.path_hash_mode {
        let _ = writeln!(&mut out, "path.hash.mode={}", config.path_hash_mode);
    }
    if config.loop_detection != defaults.loop_detection {
        let _ = writeln!(&mut out, "loop.detect={}", config.loop_detection.as_str());
    }
    config
        .extension
        .encode_changed(&defaults.extension, redact_secrets, &mut out);

    out.into_bytes()
}

fn encode_full_config_text_redacted<C: ExtensionConfig>(
    config: &StoredAppConfig<C>,
    redact_secrets: bool,
) -> Vec<u8> {
    let mut out = String::new();
    let _ = writeln!(&mut out, "# MCRS app.conf");
    let _ = writeln!(&mut out, "version={}", APP_CONFIG_TEXT_VERSION);

    let defaults = StoredAppConfig::<C>::default_with_private_key(config.private_key);
    config.acl.write_overrides(&defaults.acl, &mut out);

    out.push_str(config.private_key.config_key());
    out.push('=');
    if redact_secrets {
        out.push_str("<redacted>");
    } else {
        for byte in config.private_key.as_bytes() {
            push_hex_byte(&mut out, *byte);
        }
    }
    out.push('\n');

    out.push_str("identity.lat=");
    write_optional_coordinate(&mut out, config.latitude_microdegrees);
    out.push('\n');

    out.push_str("identity.lon=");
    write_optional_coordinate(&mut out, config.longitude_microdegrees);
    out.push('\n');

    out.push_str("node.name=");
    write_escaped_value(&mut out, &config.node_name);
    out.push('\n');

    out.push_str("owner.info=");
    write_escaped_value(&mut out, &config.owner_info);
    out.push('\n');

    if redact_secrets {
        out.push_str("remote.password=<redacted>");
    } else {
        out.push_str("remote.password=");
        write_escaped_value(&mut out, &config.remote_cli_password);
    }
    out.push('\n');

    let _ = writeln!(
        &mut out,
        "radio.frequency_hz={}",
        config.radio.receive_frequency_hz
    );
    let _ = writeln!(&mut out, "radio.bandwidth_hz={}", config.radio.bandwidth_hz);
    let _ = writeln!(
        &mut out,
        "radio.spreading_factor={}",
        config.radio.spreading_factor
    );
    let _ = writeln!(
        &mut out,
        "radio.coding_rate={}",
        config.radio.coding_rate_denominator
    );
    let _ = writeln!(
        &mut out,
        "radio.tx_power_dbm={}",
        config.radio.transmit_power_dbm
    );
    let _ = writeln!(
        &mut out,
        "radio.duty_cycle_percent={}",
        config.duty_cycle_percent
    );
    config.regions.write_config_lines(&mut out);
    let _ = writeln!(&mut out, "repeat={}", repeat_text(config.repeat_enabled));
    let _ = writeln!(
        &mut out,
        "region.capture={}",
        bool_text(config.region_capture)
    );
    let _ = writeln!(
        &mut out,
        "flood.max.unscoped={}",
        config.flood_max_unscoped_hops
    );
    let _ = writeln!(
        &mut out,
        "flood.max.advert={}",
        config.flood_max_advert_hops
    );
    let _ = writeln!(
        &mut out,
        "advert.interval={}",
        config.advert_interval_minutes
    );
    let _ = writeln!(
        &mut out,
        "flood.advert.interval={}",
        config.flood_advert_interval_hours
    );
    let _ = writeln!(&mut out, "path.hash.mode={}", config.path_hash_mode);
    let _ = writeln!(&mut out, "loop.detect={}", config.loop_detection.as_str());
    config.extension.encode_all(redact_secrets, &mut out);

    out.into_bytes()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigError {
    UnknownSetting,
    InvalidName,
    InvalidAcl,
    InvalidLatitude,
    InvalidLongitude,
    InvalidFrequency,
    InvalidBandwidth,
    InvalidSpreadingFactor,
    InvalidCodingRate,
    InvalidTransmitPower,
    InvalidDutyCycle,
    InvalidFloodMaxHops,
    InvalidPathHashMode,
    InheritedRegion,
    Region(RegionError),
    /// An invalid extension setting, with its message.
    Extension(&'static str),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::UnknownSetting => f.write_str("unknown setting"),
            ConfigError::InvalidAcl => f.write_str("invalid ACL rule or config full"),
            ConfigError::InvalidName => f.write_str("invalid name"),
            ConfigError::InvalidLatitude => f.write_str("invalid latitude"),
            ConfigError::InvalidLongitude => f.write_str("invalid longitude"),
            ConfigError::InvalidFrequency => f.write_str("invalid frequency"),
            ConfigError::InvalidBandwidth => f.write_str("invalid bandwidth"),
            ConfigError::InvalidSpreadingFactor => f.write_str("invalid spreading factor"),
            ConfigError::InvalidCodingRate => f.write_str("invalid coding rate"),
            ConfigError::InvalidTransmitPower => f.write_str("invalid transmit power"),
            ConfigError::InvalidDutyCycle => f.write_str("invalid duty cycle"),
            ConfigError::InvalidFloodMaxHops => f.write_str("invalid flood max hops"),
            ConfigError::InvalidPathHashMode => f.write_str("invalid path hash mode"),
            ConfigError::InheritedRegion => {
                f.write_str("inherited region cannot be removed; use region denyf")
            }
            ConfigError::Region(error) => write!(f, "region: {}", error),
            ConfigError::Extension(message) => f.write_str(message),
        }
    }
}

impl From<RegionError> for ConfigError {
    fn from(error: RegionError) -> Self {
        Self::Region(error)
    }
}

fn fit_node_name(input: &str) -> Result<String, ConfigError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(ConfigError::InvalidName);
    }

    let mut output = String::new();
    for character in input.chars() {
        if matches!(character, '[' | ']' | '\\' | ':' | ',' | '?' | '*') {
            return Err(ConfigError::InvalidName);
        }
        if output.len() + character.len_utf8() > MAX_NODE_NAME_LEN {
            break;
        }
        output.push(character);
    }

    if output.is_empty() {
        Err(ConfigError::InvalidName)
    } else {
        Ok(output)
    }
}

fn fit_owner_info(input: &str) -> String {
    let mut output = String::new();
    for character in input.chars() {
        if character == '\0' || output.len() + character.len_utf8() > MAX_OWNER_INFO_LEN {
            break;
        }
        output.push(if character == '|' { '\n' } else { character });
    }
    output
}

fn fit_password(input: &str) -> String {
    let mut output = String::new();

    for character in input.chars() {
        if output.len() + character.len_utf8() > MAX_REMOTE_PASSWORD_LEN {
            break;
        }
        output.push(character);
    }

    if output.is_empty() {
        output.push_str(identity::REMOTE_CLI_PASSWORD);
    }

    output
}

/// Writes a config value, escaping the characters that would end its line.
pub fn write_escaped_value(out: &mut String, value: &str) {
    for character in value.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(character),
        }
    }
}

fn unescape_value(input: &str) -> Result<String, ()> {
    let mut output = String::new();
    let mut escaped = false;

    for character in input.chars() {
        if escaped {
            match character {
                '\\' => output.push('\\'),
                'n' => output.push('\n'),
                'r' => output.push('\r'),
                _ => return Err(()),
            }
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else {
            output.push(character);
        }
    }

    if escaped { Err(()) } else { Ok(output) }
}

fn push_hex_byte(out: &mut String, byte: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push(HEX[(byte >> 4) as usize] as char);
    out.push(HEX[(byte & 0x0f) as usize] as char);
}

pub fn parse_repeat(input: &str) -> Option<bool> {
    match input.trim() {
        "on" => Some(true),
        "off" => Some(false),
        _ => None,
    }
}

pub fn repeat_text(enabled: bool) -> &'static str {
    if enabled { "on" } else { "off" }
}

pub fn parse_bool(input: &str) -> Option<bool> {
    match input.trim() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

pub fn parse_flood_max_hops(input: &str) -> Option<u8> {
    let hops = input.trim().parse::<u8>().ok()?;
    validate_flood_max_hops(hops).ok()?;
    Some(hops)
}

pub fn parse_path_hash_mode(input: &str) -> Option<u8> {
    let mode = input.trim().parse::<u8>().ok()?;
    validate_path_hash_mode(mode).ok()?;
    Some(mode)
}

pub fn bool_text(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

pub fn parse_latitude_microdegrees(input: &str) -> Option<Option<i32>> {
    parse_optional_coordinate(input, MIN_LATITUDE_MICRODEGREES, MAX_LATITUDE_MICRODEGREES)
}

pub fn parse_longitude_microdegrees(input: &str) -> Option<Option<i32>> {
    parse_optional_coordinate(
        input,
        MIN_LONGITUDE_MICRODEGREES,
        MAX_LONGITUDE_MICRODEGREES,
    )
}

fn parse_optional_coordinate(input: &str, min: i32, max: i32) -> Option<Option<i32>> {
    let input = input.trim();
    if input.is_empty() || matches!(input, "none" | "null" | "<null>") {
        return Some(None);
    }

    let value = parse_coordinate_microdegrees(input)?;
    if (min..=max).contains(&value) {
        Some(Some(value))
    } else {
        None
    }
}

fn parse_coordinate_microdegrees(input: &str) -> Option<i32> {
    let (negative, input) = match input.as_bytes().first().copied() {
        Some(b'-') => (true, &input[1..]),
        Some(b'+') => (false, &input[1..]),
        _ => (false, input),
    };
    if input.is_empty() {
        return None;
    }

    let mut whole = 0i64;
    let mut fraction = 0i64;
    let mut fraction_digits = 0usize;
    let mut seen_dot = false;
    let mut saw_digit = false;

    for byte in input.bytes() {
        match byte {
            b'0'..=b'9' if !seen_dot => {
                saw_digit = true;
                whole = whole.checked_mul(10)?.checked_add(i64::from(byte - b'0'))?;
            }
            b'0'..=b'9' => {
                saw_digit = true;
                if fraction_digits < 6 {
                    fraction = fraction
                        .checked_mul(10)?
                        .checked_add(i64::from(byte - b'0'))?;
                    fraction_digits += 1;
                }
            }
            b'.' if !seen_dot => {
                seen_dot = true;
            }
            _ => return None,
        }
    }
    if !saw_digit {
        return None;
    }

    while fraction_digits < 6 {
        fraction = fraction.checked_mul(10)?;
        fraction_digits += 1;
    }

    let value = whole
        .checked_mul(i64::from(COORDINATE_SCALE))?
        .checked_add(fraction)?;
    let value = if negative { -value } else { value };
    i32::try_from(value).ok()
}

fn validate_latitude(latitude_microdegrees: i32) -> Result<(), ConfigError> {
    if (MIN_LATITUDE_MICRODEGREES..=MAX_LATITUDE_MICRODEGREES).contains(&latitude_microdegrees) {
        Ok(())
    } else {
        Err(ConfigError::InvalidLatitude)
    }
}

fn validate_longitude(longitude_microdegrees: i32) -> Result<(), ConfigError> {
    if (MIN_LONGITUDE_MICRODEGREES..=MAX_LONGITUDE_MICRODEGREES).contains(&longitude_microdegrees) {
        Ok(())
    } else {
        Err(ConfigError::InvalidLongitude)
    }
}

fn validate_flood_max_hops(hops: u8) -> Result<(), ConfigError> {
    if hops <= 63 {
        Ok(())
    } else {
        Err(ConfigError::InvalidFloodMaxHops)
    }
}

fn validate_path_hash_mode(mode: u8) -> Result<(), ConfigError> {
    if mode <= 2 {
        Ok(())
    } else {
        Err(ConfigError::InvalidPathHashMode)
    }
}

fn write_optional_coordinate(out: &mut String, coordinate_microdegrees: Option<i32>) {
    let Some(coordinate) = coordinate_microdegrees else {
        return;
    };

    let coordinate = i64::from(coordinate);
    if coordinate < 0 {
        out.push('-');
    }
    let absolute = coordinate.abs();
    let whole = absolute / i64::from(COORDINATE_SCALE);
    let fraction = absolute % i64::from(COORDINATE_SCALE);
    let _ = write!(out, "{}.{:06}", whole, fraction);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> StoredAppConfig<()> {
        StoredAppConfig::default_with_identity_seed([7; 32])
    }

    fn network_defaults(text: &str) -> StoredAppConfig<()> {
        StoredAppConfig::with_defaults_text(PrivateKey::Seed([7; 32]), text).unwrap()
    }

    fn generated_defaults(seed: [u8; 32]) -> AppConfig<()> {
        AppConfig::generated_defaults(seed)
    }

    #[test]
    fn embedded_defaults_are_valid() {
        // Profiles may set extension settings, so firmware checks the selected profile.
        assert!(
            StoredAppConfig::<()>::with_defaults_text(PrivateKey::Seed([7; 32]), "").is_some()
        );
    }

    #[test]
    fn advert_interval_clamps_minutes_and_preserves_off() {
        let image = network_defaults("");
        for (minutes, expected) in [
            (0, 0),
            (1, 60),
            (59, 60),
            (60, 60),
            (61, 61),
            (u32::MAX, u32::MAX),
        ] {
            let text = alloc::format!("advert.interval={minutes}\n");
            let stored = decode_config_text(text.as_bytes(), &image).unwrap();
            assert_eq!(stored.advert_interval_minutes, expected);
            assert_eq!(network_defaults(&text).advert_interval_minutes, expected);
            let mut app = AppConfig::from_stored(image.clone(), "test");
            app.set_advert_interval_minutes(minutes);
            assert_eq!(app.advert_interval_minutes(), expected);
            let saved = encode_sparse_config_text(&StoredAppConfig::from_app_config(&app), &image);
            assert_eq!(
                decode_config_text(&saved, &image)
                    .unwrap()
                    .advert_interval_minutes,
                expected
            );
        }
    }

    #[test]
    fn advert_interval_defaults_and_round_trip() {
        let image = network_defaults("");
        assert_eq!(image.advert_interval_minutes, 240);
        assert_eq!(
            decode_config_text(b"version=1\n", &image)
                .unwrap()
                .advert_interval_minutes,
            240
        );
        let device = decode_config_text(b"advert.interval=60\n", &image).unwrap();
        let saved = encode_sparse_config_text(&device, &image);
        assert!(
            core::str::from_utf8(&saved)
                .unwrap()
                .contains("advert.interval=60\n")
        );
        let loaded = decode_config_text(&saved, &image).unwrap();
        assert_eq!(loaded.advert_interval_minutes, 60);
        let full = encode_full_config_text_redacted(&loaded, false);
        assert!(
            core::str::from_utf8(&full)
                .unwrap()
                .contains("advert.interval=60\n")
        );
        for value in ["-1", "1.5", "4294967296", "invalid"] {
            let text = alloc::format!("advert.interval={value}\n");
            assert!(decode_config_text(text.as_bytes(), &image).is_none());
        }
        let mut app = AppConfig::from_stored(loaded, "test");
        app.set_advert_interval_minutes(0);
        assert_eq!(app.advert_interval_minutes(), 0);
        app.unset("advert.interval").unwrap();
        assert_eq!(
            app.advert_interval_minutes(),
            defaults().advert_interval_minutes
        );
    }

    #[test]
    fn flood_advert_interval_preserves_whole_hours_and_off() {
        let image = network_defaults("");
        for (hours, expected) in [
            (0, 0),
            (1, 1),
            (59, 59),
            (60, 60),
            (61, 61),
            (u32::MAX, u32::MAX),
        ] {
            let text = alloc::format!("flood.advert.interval={hours}\n");
            let stored = decode_config_text(text.as_bytes(), &image).unwrap();
            assert_eq!(stored.flood_advert_interval_hours, expected);
            assert_eq!(
                network_defaults(&text).flood_advert_interval_hours,
                expected
            );
            let mut app = AppConfig::from_stored(image.clone(), "test");
            app.set_flood_advert_interval_hours(hours);
            assert_eq!(app.flood_advert_interval_hours(), expected);
            let saved = encode_sparse_config_text(&StoredAppConfig::from_app_config(&app), &image);
            assert_eq!(
                decode_config_text(&saved, &image)
                    .unwrap()
                    .flood_advert_interval_hours,
                expected
            );
        }
    }

    #[test]
    fn flood_advert_interval_defaults_and_round_trip() {
        let image = network_defaults("");
        assert_eq!(image.flood_advert_interval_hours, 0);
        assert_eq!(
            decode_config_text(b"version=1\n", &image)
                .unwrap()
                .flood_advert_interval_hours,
            0
        );
        let device = decode_config_text(b"flood.advert.interval=1\n", &image).unwrap();
        let saved = encode_sparse_config_text(&device, &image);
        assert!(
            core::str::from_utf8(&saved)
                .unwrap()
                .contains("flood.advert.interval=1\n")
        );
        let loaded = decode_config_text(&saved, &image).unwrap();
        assert_eq!(loaded.flood_advert_interval_hours, 1);
        let full = encode_full_config_text_redacted(&loaded, false);
        assert!(
            core::str::from_utf8(&full)
                .unwrap()
                .contains("flood.advert.interval=1\n")
        );
        for value in ["-1", "1.5", "4294967296", "invalid"] {
            let text = alloc::format!("flood.advert.interval={value}\n");
            assert!(decode_config_text(text.as_bytes(), &image).is_none());
        }
        let mut app = AppConfig::from_stored(loaded, "test");
        app.set_flood_advert_interval_hours(0);
        assert_eq!(app.flood_advert_interval_hours(), 0);
        app.unset("flood.advert.interval").unwrap();
        assert_eq!(
            app.flood_advert_interval_hours(),
            defaults().flood_advert_interval_hours
        );
    }

    #[test]
    fn repeat_defaults_round_trips_and_resets() {
        let image = network_defaults("");
        assert!(image.repeat_enabled);
        assert!(
            decode_config_text(b"version=1\n", &image)
                .unwrap()
                .repeat_enabled
        );
        for enabled in [false, true] {
            let mut app = AppConfig::from_stored(image.clone(), "test");
            app.set_repeat_enabled(enabled);
            assert_eq!(app.repeat_enabled(), enabled);
            let stored = StoredAppConfig::from_app_config(&app);
            let saved = encode_sparse_config_text(&stored, &image);
            assert_eq!(
                decode_config_text(&saved, &image).unwrap().repeat_enabled,
                enabled
            );
            let full = encode_full_config_text_redacted(&stored, false);
            assert!(
                core::str::from_utf8(&full)
                    .unwrap()
                    .contains(&alloc::format!("repeat={}\n", repeat_text(enabled)))
            );
            app.unset("repeat").unwrap();
            assert_eq!(app.repeat_enabled(), defaults().repeat_enabled);
        }
        let disabled_image = network_defaults("repeat=off\n");
        assert!(!disabled_image.repeat_enabled);
        // A saved override must survive a change to the image default.
        let enabled = decode_config_text(b"repeat=on\n", &disabled_image).unwrap();
        let saved = encode_sparse_config_text(&enabled, &disabled_image);
        assert!(
            decode_config_text(&saved, &disabled_image)
                .unwrap()
                .repeat_enabled
        );
        for invalid in ["", "of", "OFF", "enabled", "typo"] {
            assert_eq!(parse_repeat(invalid), None);
            let text = alloc::format!("repeat={invalid}\n");
            assert!(decode_config_text(text.as_bytes(), &image).is_none());
        }
    }

    #[test]
    fn delta_export_compares_profile_defaults_and_redacts_changed_secrets() {
        let image = network_defaults(
            "node.name=Profile repeater\nflood.max.advert=7\nremote.password=profile-secret\n",
        );
        let unchanged = encode_sparse_config_text_redacted(&image, &image, true);
        assert_eq!(
            core::str::from_utf8(&unchanged).unwrap(),
            "# MCRS app.conf\nversion=1\n"
        );
        let device = decode_config_text(
            b"node.name=My repeater\nflood.max.advert=3\nremote.password=device-secret\n",
            &image,
        )
        .unwrap();
        let delta = encode_sparse_config_text_redacted(&device, &image, true);
        let delta = core::str::from_utf8(&delta).unwrap();
        assert!(delta.contains("node.name=My repeater\n"));
        // The base default still counts as a change when the profile differs.
        assert!(delta.contains("flood.max.advert=3\n"));
        assert!(delta.contains("remote.password=<redacted>\n"));
        assert!(!delta.contains("device-secret"));
        assert!(!delta.contains("identity.seed="));
        assert!(!delta.contains("identity.expanded="));
        assert!(!delta.contains("radio.frequency_hz="));
        // Saving still preserves identity and secrets for an exact reload.
        let saved = encode_sparse_config_text(&device, &image);
        let loaded = decode_config_text(&saved, &image).unwrap();
        assert_eq!(loaded.private_key, device.private_key);
        assert_eq!(loaded.remote_cli_password, device.remote_cli_password);
    }

    #[test]
    fn saved_differences_survive_changes_to_image_defaults() {
        let image = network_defaults("flood.max.unscoped=4\nflood.max.advert=2\n");
        let device =
            decode_config_text(b"flood.max.unscoped=4\nflood.max.advert=1\n", &image).unwrap();
        let saved = encode_sparse_config_text(&device, &image);
        let text = core::str::from_utf8(&saved).unwrap();
        assert!(!text.contains("flood.max.unscoped="));
        assert!(text.contains("flood.max.advert=1"));

        let updated = network_defaults("flood.max.unscoped=3\nflood.max.advert=2\n");
        let loaded = decode_config_text(&saved, &updated).unwrap();
        assert_eq!(loaded.flood_max_unscoped_hops, 3);
        assert_eq!(loaded.flood_max_advert_hops, 1);
        assert_eq!(loaded.private_key, device.private_key);

        let matching = network_defaults("flood.max.unscoped=3\nflood.max.advert=1\n");
        let saved = encode_sparse_config_text(&loaded, &matching);
        assert!(!core::str::from_utf8(&saved).unwrap().contains("flood.max."));
    }

    #[test]
    fn region_overrides_are_additive_and_only_differences_are_saved() {
        let image = network_defaults(
            "region.default=northeast\nregion.northeast=true\nregion.other=true\n",
        );
        let device =
            decode_config_text(b"region.local=true\nregion.northeast=false\n", &image).unwrap();
        let saved = encode_sparse_config_text(&device, &image);
        let text = core::str::from_utf8(&saved).unwrap();
        assert!(text.contains("region.northeast=false\n"));
        assert!(text.contains("region.local=true\n"));
        assert!(!text.contains("region.other="));
        assert!(!text.contains("region.default="));

        let updated = network_defaults(
            "region.default=northeast\nregion.northeast=true\nregion.other=false\nregion.new=true\n",
        );
        let loaded = decode_config_text(&saved, &updated).unwrap();
        let allowed = |name| {
            loaded
                .regions
                .find_by_name_prefix(name)
                .map(|region| region.allows_flood())
        };
        assert_eq!(allowed("northeast"), Some(false));
        assert_eq!(allowed("local"), Some(true));
        assert_eq!(allowed("other"), Some(false));
        assert_eq!(allowed("new"), Some(true));
        assert_eq!(
            loaded.regions.default_region().unwrap().display_name(),
            "northeast"
        );
    }

    #[test]
    fn profile_parser_rejects_invalid_settings_and_device_keys() {
        for text in [
            String::from("typo=1"),
            String::from("radio.spreading_factor=99"),
            String::from("region.northeast=invalid"),
            alloc::format!("identity.seed={}", "00".repeat(32)),
        ] {
            assert!(
                StoredAppConfig::<()>::with_defaults_text(PrivateKey::Seed([7; 32]), &text)
                    .is_none()
            );
        }
    }

    #[test]
    fn loop_detection_defaults_round_trips_and_resets() {
        let mut config = generated_defaults([7; 32]);
        assert_eq!(config.loop_detection(), LoopDetection::Minimal);
        let legacy = decode_config_text(b"version=1\n", &defaults()).unwrap();
        assert_eq!(legacy.loop_detection, LoopDetection::Minimal);
        for mode in [
            LoopDetection::Off,
            LoopDetection::Minimal,
            LoopDetection::Moderate,
            LoopDetection::Strict,
        ] {
            config.set_loop_detection(mode);
            let stored = StoredAppConfig::from_app_config(&config);
            for encoded in [
                encode_config_text(&stored),
                encode_full_config_text_redacted(&stored, false),
            ] {
                let decoded = decode_config_text(&encoded, &defaults()).unwrap();
                assert_eq!(
                    AppConfig::from_stored(decoded, "storage").loop_detection(),
                    mode
                );
            }
        }
        config.unset("loop.detect").unwrap();
        assert_eq!(config.loop_detection(), LoopDetection::Minimal);
        let encoded = encode_config_text(&StoredAppConfig::from_app_config(&config));
        assert!(
            !core::str::from_utf8(&encoded)
                .unwrap()
                .contains("loop.detect=")
        );
        assert!(decode_config_text(b"version=1\nloop.detect=unknown\n", &defaults()).is_none());
    }

    #[test]
    fn owner_info_round_trips_and_clears() {
        let mut config = generated_defaults([7; 32]);
        assert_eq!(config.owner_info(), "");
        config.set_owner_info("Alice|alice@example.org|Back\\yard");
        let stored = StoredAppConfig::from_app_config(&config);
        for encoded in [
            encode_config_text(&stored),
            encode_full_config_text_redacted(&stored, false),
        ] {
            let decoded = decode_config_text(&encoded, &defaults()).unwrap();
            let reloaded = AppConfig::from_stored(decoded, "storage");
            assert_eq!(
                reloaded.owner_info(),
                "Alice\nalice@example.org\nBack\\yard"
            );
        }
        config.unset("owner.info").unwrap();
        let encoded = encode_config_text(&StoredAppConfig::from_app_config(&config));
        assert!(
            !core::str::from_utf8(&encoded)
                .unwrap()
                .contains("owner.info=")
        );
        assert_eq!(
            decode_config_text(&encoded, &defaults())
                .unwrap()
                .owner_info,
            ""
        );
    }

    #[test]
    fn owner_info_limits_bytes_without_splitting_utf8() {
        assert_eq!(fit_owner_info(&"x".repeat(120)).len(), 119);
        assert_eq!(fit_owner_info(&("x".repeat(118) + "é")), "x".repeat(118));
        assert_eq!(fit_owner_info(&("x".repeat(117) + "é")).len(), 119);
        assert_eq!(fit_owner_info("Alice\0ignored"), "Alice");
    }

    fn expanded_key() -> PrivateKey {
        PrivateKey::from_hex("28ad39fefd7fa3e200a9c626eef599e61a2d055c48a8288a4e7e4c4bca3928789c7d6db3506d65dbac7c052aaee4857425210c9bc54030c826e54055983452a5").unwrap()
    }

    #[test]
    fn private_key_formats_round_trip_and_redact() {
        for key in [PrivateKey::Seed([7; 32]), expanded_key()] {
            let stored = StoredAppConfig::<()>::default_with_private_key(key);
            let encoded = encode_config_text(&stored);
            let text = core::str::from_utf8(&encoded).unwrap();
            assert!(text.contains(key.config_key()));
            let decoded = decode_config_text(&encoded, &defaults()).unwrap();
            assert_eq!(decoded.private_key, key);
            assert_eq!(decoded.node_name, stored.node_name);
            let redacted = encode_full_config_text_redacted(&stored, true);
            let redacted = core::str::from_utf8(&redacted).unwrap();
            assert!(redacted.contains(&alloc::format!("{}=<redacted>", key.config_key())));
            let secret_line = text
                .lines()
                .find(|line| line.starts_with(key.config_key()))
                .unwrap();
            assert!(!redacted.contains(secret_line));
        }
    }

    #[test]
    fn importing_key_keeps_active_identity_until_reload() {
        let mut config = generated_defaults([9; 32]);
        let original_public = *config.identity().public_key();
        let original_signature = config.identity().sign(b"advert");
        config.set_private_key(expanded_key());
        assert_eq!(config.identity().public_key(), &original_public);
        assert_eq!(config.identity().sign(b"advert"), original_signature);
        config.unset("name").unwrap();
        assert_eq!(*config.private_key(), expanded_key());
        let encoded = encode_config_text(&StoredAppConfig::from_app_config(&config));
        let reloaded = AppConfig::from_stored(
            decode_config_text(&encoded, &defaults()).unwrap(),
            "storage",
        );
        assert_eq!(
            reloaded.identity().public_key(),
            Identity::from_private_key(expanded_key()).public_key()
        );
        assert_eq!(reloaded.node_name(), "Repeater-ea4a6c");
    }

    #[test]
    fn rejects_keys_with_wrong_config_format() {
        let expanded = encode_config_text(&StoredAppConfig::<()>::default_with_private_key(
            expanded_key(),
        ));
        let wrong = core::str::from_utf8(&expanded)
            .unwrap()
            .replace("identity.expanded=", "identity.seed=");
        assert!(decode_config_text(wrong.as_bytes(), &defaults()).is_none());
        let seed = encode_config_text(&defaults());
        let wrong = core::str::from_utf8(&seed)
            .unwrap()
            .replace("identity.seed=", "identity.expanded=");
        assert!(decode_config_text(wrong.as_bytes(), &defaults()).is_none());
    }

    #[test]
    fn replacing_private_key_only_persists_the_selected_format() {
        let mut config = generated_defaults([9; 32]);
        for key in [expanded_key(), PrivateKey::Seed([7; 32])] {
            config.set_private_key(key);
            let encoded = encode_config_text(&StoredAppConfig::from_app_config(&config));
            let text = core::str::from_utf8(&encoded).unwrap();
            assert_eq!(
                text.lines()
                    .filter(|line| line.starts_with("identity.seed=")
                        || line.starts_with("identity.expanded="))
                    .count(),
                1
            );
            assert_eq!(
                decode_config_text(&encoded, &defaults())
                    .unwrap()
                    .private_key,
                key
            );
        }
    }

    /// A stand-in for firmware settings with a secret and a flag.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    struct TestExtension {
        token: String,
        enabled: bool,
    }

    impl ExtensionConfig for TestExtension {
        fn decode(
            &mut self,
            key: &str,
            value: String,
            source: ConfigSource,
        ) -> Option<Result<(), ()>> {
            match key {
                "test.token" => self.token = value,
                "test.enabled" => match parse_bool(&value) {
                    Some(enabled) => self.enabled = enabled,
                    None => return Some(Err(())),
                },
                _ if source == ConfigSource::Defaults => {}
                _ => return None,
            }
            Some(Ok(()))
        }

        fn validate(&self) -> bool {
            self.token != "invalid"
        }

        fn encode_changed(&self, defaults: &Self, redact_secrets: bool, out: &mut String) {
            if self.token != defaults.token {
                out.push_str("test.token=");
                if redact_secrets {
                    out.push_str("<redacted>");
                } else {
                    write_escaped_value(out, &self.token);
                }
                out.push('\n');
            }
            if self.enabled != defaults.enabled {
                let _ = writeln!(out, "test.enabled={}", bool_text(self.enabled));
            }
        }

        fn encode_all(&self, redact_secrets: bool, out: &mut String) {
            out.push_str("test.token=");
            if redact_secrets {
                out.push_str("<redacted>");
            } else {
                write_escaped_value(out, &self.token);
            }
            out.push('\n');
            let _ = writeln!(out, "test.enabled={}", bool_text(self.enabled));
        }

        fn unset(&mut self, setting: &str, defaults: &Self) -> Result<(), ConfigError> {
            match setting {
                "test.token" => self.token = defaults.token.clone(),
                "test.enabled" => self.enabled = defaults.enabled,
                _ => return Err(ConfigError::UnknownSetting),
            }
            Ok(())
        }
    }

    #[test]
    fn extension_settings_are_layered_saved_and_reset() {
        let image = StoredAppConfig::<TestExtension>::with_defaults_text(
            PrivateKey::Seed([7; 32]),
            "test.enabled=true\n",
        )
        .unwrap();
        assert!(image.extension.enabled);
        // Unknown profile settings are still rejected, and invalid values fail the layer.
        for profile in ["test.typo=1\n", "test.enabled=maybe\n", "test.token=invalid\n"] {
            assert!(
                StoredAppConfig::<TestExtension>::with_defaults_text(
                    PrivateKey::Seed([7; 32]),
                    profile
                )
                .is_none()
            );
        }
        // Stored settings from removed features are ignored rather than rejected.
        let device = decode_config_text(
            b"test.token=line\\nbreak\ntest.enabled=false\nold.feature=1\n",
            &image,
        )
        .unwrap();
        assert_eq!(device.extension.token, "line\nbreak");
        assert!(!device.extension.enabled);

        let saved = encode_sparse_config_text(&device, &image);
        let text = core::str::from_utf8(&saved).unwrap();
        assert!(text.contains("test.token=line\\nbreak\n"));
        assert!(text.contains("test.enabled=false\n"));
        assert!(!text.contains("old.feature"));
        assert_eq!(
            decode_config_text(&saved, &image).unwrap().extension,
            device.extension
        );
        let delta = encode_sparse_config_text_redacted(&device, &image, true);
        assert!(
            core::str::from_utf8(&delta)
                .unwrap()
                .contains("test.token=<redacted>\n")
        );
        let full = encode_full_config_text_redacted(&device, true);
        let full = core::str::from_utf8(&full).unwrap();
        assert!(full.contains("test.token=<redacted>\ntest.enabled=false\n"));
        assert!(!full.contains("break"));

        let mut app = AppConfig::from_stored(device, "test");
        app.unset("test.token").unwrap();
        assert_eq!(app.extension().token, "");
        assert_eq!(app.unset("test.typo"), Err(ConfigError::UnknownSetting));
    }
}
