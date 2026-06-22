#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use esp_hal::clock::CpuClock;
use esp_hal::peripherals;
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::rmt::{Channel, PulseCode, Rmt, Tx, TxChannelConfig, TxChannelCreator};
use esp_hal::Async;

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer, Instant};

use static_cell::StaticCell;

use esp32_phyllotaxis::util::*;
use esp32_phyllotaxis::sketches::*;
use esp32_phyllotaxis::audio::*;

use esp_println as _;
use defmt::info;
use defmt::error;
use defmt::unwrap;

#[panic_handler]
fn panic(panic_info: &core::panic::PanicInfo) -> ! {
    error!("{}", panic_info);
    loop {}
}

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

const T0: PulseCode = PulseCode::new(Level::High, 32, Level::Low, 68);
const T1: PulseCode = PulseCode::new(Level::High, 64, Level::Low, 36);

static TX_BUFFER: StaticCell<[PulseCode; NUM_LEDS * 24 + 1]> = StaticCell::new();

fn encode(rgb: [Rgb; NUM_LEDS], tx: &mut [PulseCode; NUM_LEDS * 24 + 1]) {
    let mut i = 0;
    for led in 0..NUM_LEDS {
        let grb = [rgb[led].g, rgb[led].r, rgb[led].b];
        for byte in grb {
            for bit in (0..8).rev() {
                tx[i] = if (byte >> bit) & 1 == 1 { T1 } else { T0 };
                i += 1;
            }
        }
    }
    tx[i] = PulseCode::end_marker();
}

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    // generator version: 1.3.0
    // generator parameters: --chip esp32s3 -o unstable-hal -o embassy -o defmt

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    let rmt = Rmt::new(peripherals.RMT, Rate::from_mhz(80)).unwrap().into_async();

    let mut channel = rmt.channel0.configure_tx(
        &TxChannelConfig::default()
        .with_clk_divider(1)
        .with_idle_output(true)
        .with_idle_output_level(Level::Low)
    ).unwrap().with_pin(peripherals.GPIO4);


    //*x = encode(Rgb::new(0,30,0));
    //channel.transmit(x).await.unwrap();

    info!("Embassy initialized!");

    // TODO: Spawn some tasks
    let _ = spawner;
    spawner.spawn(unwrap!(render(channel)));

    loop {
        info!("Hello world!");
        Timer::after(Duration::from_secs(1)).await;
    }

    // for inspiration have a look at the examples at https://github.com/esp-rs/esp-hal/tree/esp-hal-v1.1.0/examples
}

#[embassy_executor::task]
async fn render(mut channel: Channel<'static, Async, Tx>) {
    let mut last = Instant::now();
    let mut t: f32 = 0.0;
    let mut bank = SketchBank::new();
    let mut next = Instant::now();
    let mut led_buf = [Rgb::new(0,0,0); NUM_LEDS];
    let tx_buf: &'static mut [PulseCode; NUM_LEDS* 24 + 1] = TX_BUFFER.init([PulseCode::end_marker(); NUM_LEDS * 24 + 1]);
    loop {
        next += Duration::from_millis(1000/120);
        let now = Instant::now();
        let dt = (now - last).as_millis() as f32 / 1000.0;
        last = now;
        t += dt;

        let audio = AudioFeatures::default();
        let params = SketchParams { dt, t, audio, released: false };
        bank.render(&mut led_buf, &params);
        apply_gamma(&mut led_buf);

        encode(led_buf, tx_buf);

        channel.transmit(tx_buf).await.unwrap();
        
        Timer::at(next).await;
    }
}
