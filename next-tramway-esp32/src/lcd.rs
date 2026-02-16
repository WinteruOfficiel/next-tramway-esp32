use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use embassy_time::{Timer, Duration};
use esp_hal::{Blocking, i2c::master::I2c};

const LCD_ADDR: u8 = 0x27;
mod lcd_bits {
    pub const EN: u8 = 0b0000_0100; 
    pub const RW: u8 = 0b0000_0010;
    pub const RS: u8 = 0b0000_0001;
    pub const BL: u8 = 0b0000_1000;
}


pub struct Lcd<'a> {
    bus: &'a Mutex<CriticalSectionRawMutex, Option<I2c<'static, Blocking>>>,
}
impl<'a> Lcd<'a> {

    pub fn new(
        bus: &'a Mutex<CriticalSectionRawMutex, Option<I2c<'static, Blocking>>>,
    ) -> Self {
        Self { bus }
    }

    pub async fn send(&self, value: u8, mode: u8) {
        let highnib = value & 0xF0;
        let lownib = (value << 4) & 0xF0;
        self.write_4_bits(highnib | mode | lcd_bits::BL).await;
        self.write_4_bits(lownib | mode | lcd_bits::BL).await;
    }

    // D7 D6 D5 D4 BL EN RW RS
    pub async fn write_4_bits(&self, value: u8) {
        let mut guard = self.bus.lock().await;
        let i2c = guard.as_mut().expect("I2C not initialized");
        self.write_i2c(i2c, value);
        self.pulse_enable(i2c, value).await;
    }

    fn write_i2c(&self, i2c_bus: &mut I2c<'_, Blocking>, data: u8) {
        let result = i2c_bus.write(LCD_ADDR, &[data]);

        if result.is_err() {
            esp_println::println!("Error when sending");
        }
    }

    async fn pulse_enable(&self, i2c_bus: &mut I2c<'_, Blocking>, data: u8) {
        self.write_i2c(i2c_bus, data | lcd_bits::EN);
        Timer::after(Duration::from_micros(1)).await;
        self.write_i2c(i2c_bus, data & !lcd_bits::EN);
        Timer::after(Duration::from_micros(50)).await;
    }

    pub async fn set_4_bits_mode(&self) {
        self.write_4_bits(0x03 << 4).await;
        Timer::after(Duration::from_micros(4500)).await;
        self.write_4_bits(0x03 << 4).await;
        Timer::after(Duration::from_micros(4500)).await;
        self.write_4_bits(0x03 << 4).await;
        Timer::after(Duration::from_micros(150)).await;
        self.write_4_bits(0x02 << 4).await;
    }
}
