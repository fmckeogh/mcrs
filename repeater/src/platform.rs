//! Services the repeater needs from the device it runs on. Firmware installs
//! its implementation once at boot, before it creates an `AppContext`.
use core::{cell::Cell, fmt};

use critical_section::Mutex;

pub mod storage;

pub trait Platform: Sync {
    /// Milliseconds since boot.
    fn now_millis(&self) -> u64;

    /// Unix time in seconds once the wall clock is known, otherwise uptime.
    fn now_seconds(&self) -> u32;

    /// Sets the wall clock unless that would move it backwards.
    fn set_wall_clock_if_forward(&self, unix_seconds: u32) -> bool;

    fn random_u32(&self) -> u32;

    fn reboot(&self) -> !;

    /// Reported by the `ver` command and in owner info, e.g. `MCRS 0.1.2`.
    fn firmware_version(&self) -> &'static str;

    fn log(&self, args: fmt::Arguments<'_>);

    /// Used until the board reports a battery sample.
    fn battery_level_percent(&self) -> Option<u8> {
        None
    }

    /// Used until the board reports a battery sample.
    fn battery_millivolts(&self) -> Option<u16> {
        None
    }
}

static PLATFORM: Mutex<Cell<Option<&'static dyn Platform>>> = Mutex::new(Cell::new(None));

pub fn install(platform: &'static dyn Platform) {
    critical_section::with(|cs| PLATFORM.borrow(cs).set(Some(platform)));
}

fn platform() -> &'static dyn Platform {
    // Copy the reference out so platform calls run outside the critical section.
    critical_section::with(|cs| PLATFORM.borrow(cs).get()).expect("platform not installed")
}

pub fn now_millis() -> u64 {
    platform().now_millis()
}

pub fn now_seconds() -> u32 {
    platform().now_seconds()
}

pub fn set_wall_clock_if_forward(unix_seconds: u32) -> bool {
    platform().set_wall_clock_if_forward(unix_seconds)
}

pub fn random_u32() -> u32 {
    platform().random_u32()
}

pub fn reboot() -> ! {
    platform().reboot()
}

pub fn firmware_version() -> &'static str {
    platform().firmware_version()
}

pub fn battery_level_percent() -> Option<u8> {
    platform().battery_level_percent()
}

pub fn battery_millivolts() -> Option<u16> {
    platform().battery_millivolts()
}

pub fn log_fmt(args: fmt::Arguments<'_>) {
    platform().log(args);
}

pub fn log_hex_line(label: &str, bytes: &[u8], max_len: usize) {
    log_fmt(format_args!("{}{}", label, HexLine(bytes, max_len)));
}

pub fn log_i8_line(label: &str, values: &[i8], max_len: usize) {
    log_fmt(format_args!("{}{}", label, I8Line(values, max_len)));
}

struct HexLine<'a>(&'a [u8], usize);

impl fmt::Display for HexLine<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0.iter().take(self.1) {
            write!(f, " {:02x}", byte)?;
        }
        if self.0.len() > self.1 {
            f.write_str(" ...")?;
        }
        Ok(())
    }
}

struct I8Line<'a>(&'a [i8], usize);

impl fmt::Display for I8Line<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for value in self.0.iter().take(self.1) {
            write!(f, " {}", value)?;
        }
        if self.0.len() > self.1 {
            f.write_str(" ...")?;
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod test {
    /// Response builders need a clock. Use a fixed time without any hardware.
    struct TestPlatform;

    impl super::Platform for TestPlatform {
        fn now_millis(&self) -> u64 {
            1_000
        }

        fn now_seconds(&self) -> u32 {
            1
        }

        fn set_wall_clock_if_forward(&self, _unix_seconds: u32) -> bool {
            false
        }

        fn random_u32(&self) -> u32 {
            4
        }

        fn reboot(&self) -> ! {
            panic!("reboot requested");
        }

        fn firmware_version(&self) -> &'static str {
            "MCRS test"
        }

        fn log(&self, _args: core::fmt::Arguments<'_>) {}
    }

    pub(crate) fn install() {
        super::install(&TestPlatform);
    }
}
