//! Measures the battery through a resistor divider on an analog input, and
//! keeps the repeater off while the battery is too low to run it.
use cortex_m::peripheral::SCB;
use defmt::{debug, error, info, warn};
use embassy_nrf::{
    Peri,
    interrupt::typelevel::{Binding, SAADC as SaadcInterrupt},
    peripherals::SAADC,
    saadc::{self, ChannelConfig, Config, Gain, Oversample, Reference, Resolution, Saadc, Time},
};
use embassy_time::{Duration, Timer, with_timeout};
use mcrs_repeater::platform::{Battery, Radio};

/// How long to wait before checking a low battery again.
const RECOVERY_SLEEP: Duration = Duration::from_secs(5 * 60);
const RADIO_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

const SAMPLES: u32 = 8;
/// The top of the ADC's range, as [`NrfBattery::new`] sets it up: 12 bits, from
/// the 0.6 V internal reference at a gain of 1/6.
const FULL_SCALE_READING: u32 = 4096;
const FULL_SCALE_MV: u32 = 3600;
/// A lower reading means there's no battery on the divider, as on USB power or
/// with a fault, rather than a flat one. Calling it failed keeps the repeater
/// running.
const MIN_PLAUSIBLE_MV: u32 = 2000;

/// Measures the battery.
pub struct NrfBattery<'d> {
    saadc: Saadc<'d, 1>,
    divider_scale_milli: u32,
}

impl<'d> NrfBattery<'d> {
    /// `sense` is the divider's output, and `divider_scale_milli` is how many
    /// times higher the battery voltage is than that output, in thousandths.
    pub fn new(
        saadc: Peri<'d, SAADC>,
        irq: impl Binding<SaadcInterrupt, saadc::InterruptHandler> + 'd,
        sense: impl saadc::Input + 'd,
        divider_scale_milli: u32,
    ) -> Self {
        let mut config = Config::default();
        config.resolution = Resolution::_12BIT;
        config.oversample = Oversample::BYPASS;

        let mut channel = ChannelConfig::single_ended(sense);
        channel.reference = Reference::INTERNAL;
        channel.gain = Gain::GAIN1_6;
        // Dividers use large resistors, which charge the ADC's input slowly.
        channel.time = Time::_40US;

        Self {
            saadc: Saadc::new(saadc, irq, config, [channel]),
            divider_scale_milli,
        }
    }
}

impl Battery for NrfBattery<'_> {
    async fn millivolts(&mut self) -> Option<u16> {
        // The ADC's offset changes with temperature.
        self.saadc.calibrate().await;

        let mut total = 0;
        for _ in 0..SAMPLES {
            let mut sample = [0];
            self.saadc.sample(&mut sample).await;
            // Noise can put a reading near zero just below it.
            total += sample[0].max(0) as u32;
        }

        let adc_millivolts = total * FULL_SCALE_MV / (SAMPLES * FULL_SCALE_READING);
        let millivolts = adc_millivolts * self.divider_scale_milli / 1000;
        debug!("Battery: adc={}mV battery={}mV", adc_millivolts, millivolts);

        if millivolts < MIN_PLAUSIBLE_MV {
            warn!("Battery: ignoring implausible reading of {}mV", millivolts);
            return None;
        }
        u16::try_from(millivolts).ok()
    }
}

/// Waits until the battery reads at least `minimum_mv`, so the repeater doesn't
/// start on a battery too low to run it. A failed reading doesn't count as low.
pub async fn wait_for_battery(battery: &mut impl Battery, minimum_mv: u16) {
    while let Some(millivolts) = battery.millivolts().await.filter(|mv| *mv < minimum_mv) {
        warn!(
            "Startup battery: {}mV (minimum {}mV)",
            millivolts, minimum_mv
        );
        Timer::after(RECOVERY_SLEEP).await;
    }
}

/// Puts the radio to sleep and reboots after a wait, for when the repeater has
/// stopped because of low voltage or the brownout command. The nRF52840 can't
/// wake itself from System OFF on a timer, so it idles instead, and
/// [`wait_for_battery`] at startup holds the repeater until the battery recovers.
pub async fn recover(radio: &mut impl Radio) -> ! {
    info!("Entering low-power recovery");
    match with_timeout(RADIO_SHUTDOWN_TIMEOUT, radio.shutdown()).await {
        Ok(Ok(())) => {}
        Ok(Err(())) => error!("Radio shutdown failed"),
        Err(_) => error!("Radio shutdown timed out"),
    }
    Timer::after(RECOVERY_SLEEP).await;
    SCB::sys_reset()
}
