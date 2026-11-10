//! MCRS repeater for the SenseCAP Solar Node P1. It has no status screen,
//! button or console.
#![no_std]
#![no_main]

use defmt_rtt as _;
use embassy_nrf::{
    bind_interrupts,
    gpio::{Input, Level, Output, OutputDrive, Pull},
    nvmc::Nvmc,
    peripherals,
    rng::Rng,
    saadc,
    spim::{self, Spim},
};
use embassy_time::{Delay, Timer};
use embedded_alloc::LlffHeap as Heap;
use embedded_hal_bus::spi::ExclusiveDevice;
use mcrs_nrf::{HEAP_SIZE, NrfBattery, NrfPlatform, NrfStorage, recover, wait_for_battery};
use mcrs_repeater::{
    Repeater,
    platform::{NoButton, NoConsole, NoDisplay},
};
use mcrs_sx1262::{BoardConfig, Sx1262, TcxoCtrlVoltage};
use panic_probe as _;

const MODEL: &str = "SenseCAP Solar Node P1";
// Provisional values; validate against this board's battery and regulator.
const STARTUP_MIN_MV: u16 = 3300;
const VOLTAGE_WARNING_MV: u16 = 3000;
// The XIAO nRF52840's own divider, 1 MΩ over 510 kΩ.
const BATTERY_DIVIDER_SCALE_MILLI: u32 = 2961;

#[global_allocator]
static HEAP: Heap = Heap::empty();

bind_interrupts!(struct Irqs {
    SAADC => saadc::InterruptHandler;
    TWISPI1 => spim::InterruptHandler<peripherals::TWISPI1>;
});

#[embassy_executor::main]
async fn main(_spawner: embassy_executor::Spawner) -> ! {
    unsafe {
        embedded_alloc::init!(HEAP, HEAP_SIZE);
    }

    let p = embassy_nrf::init(Default::default());

    let nss = Output::new(p.P0_04, Level::High, OutputDrive::Standard);
    let reset = Output::new(p.P0_28, Level::High, OutputDrive::Standard);
    let dio1 = Input::new(p.P0_03, Pull::Down);
    let busy = Input::new(p.P0_29, Pull::None);

    // Low connects the battery divider. It stays low, as in MeshCore and Meshtastic, because
    // high can put too much voltage on the sense pin. MeshCore gives it 50ms to settle.
    let _battery_enable = Output::new(p.P0_14, Level::Low, OutputDrive::Standard);
    Timer::after_millis(50).await;
    let mut battery = NrfBattery::new(p.SAADC, Irqs, p.P0_31, BATTERY_DIVIDER_SCALE_MILLI);
    // Before anything that draws much power or writes to the flash.
    wait_for_battery(&mut battery, STARTUP_MIN_MV).await;

    let mut spi_config = spim::Config::default();
    spi_config.frequency = spim::Frequency::M16;
    let spim = Spim::new(p.TWISPI1, Irqs, p.P1_13, p.P1_14, p.P1_15, spi_config);
    let spi = ExclusiveDevice::new(spim, nss, Delay).unwrap();

    let mut radio = Sx1262::new(
        spi,
        reset,
        dio1,
        busy,
        Delay,
        BoardConfig {
            tcxo_ctrl: Some(TcxoCtrlVoltage::Ctrl1V7),
            use_dcdc: true,
            rx_boost: false,
        },
    );
    let platform = NrfPlatform::new(MODEL, Rng::new_blocking(p.RNG));
    let storage = NrfStorage::new(Nvmc::new(p.NVMC));

    let mut console = NoConsole;
    let mut display = NoDisplay;
    let mut button = NoButton;

    Repeater::new(
        platform,
        storage,
        &mut radio,
        &mut console,
        &mut display,
        &mut button,
        &mut battery,
    )
    .run(VOLTAGE_WARNING_MV)
    .await;

    recover(&mut radio).await
}
