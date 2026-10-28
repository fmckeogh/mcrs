//! The radio interface used by the repeater. Signal units: RSSI in dBm, SNR in quarter-dB.
use core::fmt;

/// Preamble length used for every packet, and for airtime calculations.
pub const RECEIVE_PREAMBLE_LENGTH: u16 = 8;

pub struct ReceivedPacket {
    pub len: usize,
    pub rssi: i16,
    pub snr_quarters: i16,
}

pub trait Receiver {
    async fn wait_for_read(&mut self, buffer: &mut [u8]) -> Result<ReceivedPacket, ()>;

    async fn channel_is_busy(&mut self) -> Result<bool, ()>;

    async fn transmit(&mut self, payload: &[u8]) -> Result<(), ()>;
}

/// Whether a packet was received from, or transmitted to, the radio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Rx,
    Tx,
}

/// Format quarter-dB as a decimal dB value without floating point or allocation.
pub struct SnrDb(pub i16);
impl fmt::Display for SnrDb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let magnitude = self.0.unsigned_abs();
        if self.0 < 0 {
            f.write_str("-")?;
        }
        write!(f, "{}.{:02}", magnitude / 4, (magnitude % 4) * 25)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;

    #[test]
    fn displays_db_including_negative_fractional_values() {
        for (quarters, expected) in [
            (32, "8.00"),
            (33, "8.25"),
            (34, "8.50"),
            (35, "8.75"),
            (-1, "-0.25"),
            (-33, "-8.25"),
            (0, "0.00"),
            (i16::MIN, "-8192.00"),
        ] {
            assert_eq!(format!("{}", SnrDb(quarters)), expected);
        }
    }
}
