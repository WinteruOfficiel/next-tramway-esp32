#![no_std]
#![no_main]

use next_tramway_esp32::lcd::Lcd;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use esp_hal::{Blocking, clock::CpuClock, i2c::master::I2c, time::Rate, timer::timg::TimerGroup};
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};

esp_bootloader_esp_idf::esp_app_desc!();

static I2C_BUS: Mutex<CriticalSectionRawMutex, Option<I2c<'static, Blocking>>> =
    Mutex::new(None);


#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

async fn scan_i2c_bus() {
    esp_println::println!("Waiting for i2c mutex...");
    let mut guard = I2C_BUS.lock().await;
    let i2c = guard.as_mut().expect("I2C not initialized");
    esp_println::println!("Scanning I2C bus...");

    for addr in 0x08..=0x77 {
        let result = i2c.write(addr, &[]);

        if result.is_ok() {
            esp_println::println!("I2C device found at 0x{:02X}", addr);
        }
    }

    esp_println::println!("Scan done.");
}

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(esp_hal::Config::default());

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    esp_println::logger::init_logger_from_env();
    esp_println::println!("Embassy init !");

    let i2c_bus = esp_hal::i2c::master::I2c::new(
        peripherals.I2C0,
        esp_hal::i2c::master::Config::default().with_frequency(Rate::from_khz(100)),
    )
        .unwrap()
        .with_scl(peripherals.GPIO11)
        .with_sda(peripherals.GPIO10);

    I2C_BUS.lock().await.replace(i2c_bus);
    
    Timer::after(Duration::from_millis(50)).await;
    scan_i2c_bus().await;

    let lcd = Lcd::new(&I2C_BUS);
    lcd.set_4_bits_mode().await;
    lcd.send(0x28, 0).await; // function set
    lcd.send(0x0C, 0).await; // display ON
    lcd.send(0x01, 0).await; // clear
    Timer::after(Duration::from_millis(2)).await;
    lcd.send(0x06, 0).await; // entry mode

    lcd.send(b'E', 1).await;




    loop {
        Timer::after(Duration::from_secs(1)).await;
    }
}

