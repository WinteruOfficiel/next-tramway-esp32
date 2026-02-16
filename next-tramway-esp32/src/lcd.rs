use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use embassy_time::{Timer, Duration};
use esp_hal::{Blocking, i2c::master::I2c};

pub enum LcdGeometry {
    L1602,
    L2004,
}



const LCD_ADDR: u8 = 0x27;
mod lcd_bits {
    pub const EN: u8 = 0b0000_0100; 
    pub const RW: u8 = 0b0000_0010;
    pub const RS: u8 = 0b0000_0001;
    pub const BL: u8 = 0b0000_1000;
}

mod lcd_commands {
    pub const LCD_SETDDRAMADDR: u8 = 0x80;
    pub const LCD_CLEARDISPLAY: u8 = 0x01;
}

pub struct Lcd<'a> {
    bus: &'a Mutex<CriticalSectionRawMutex, Option<I2c<'static, Blocking>>>,
    geom: LcdGeometry,
    curr_row: u8,
    curr_col: u8
}
impl<'a> Lcd<'a> {

    pub fn new(
        bus: &'a Mutex<CriticalSectionRawMutex, Option<I2c<'static, Blocking>>>,
    ) -> Self {
        Self { bus, geom: LcdGeometry::L1602, curr_row: 0, curr_col: 0 }
    }

    pub async fn init(&self) {
        self.set_4_bits_mode().await;
        self.send(0x28, 0).await; // function set
        self.send(0x0C, 0).await; // display ON
        self.send(0x01, 0).await; // clear
        Timer::after(Duration::from_millis(2)).await;
        self.send(0x06, 0).await; // entry mode
    }

    fn get_size_and_offset(&self) -> (u8, u8, &[u8]) {
        match self.geom {
            LcdGeometry::L1602 => (1, 15, &[0x00, 0x40][..]),
            LcdGeometry::L2004 => todo!(),
        }
    }

    pub async fn set_cursor(&mut self, mut row:  u8, mut col: u8) {
        let (max_row, max_col, offsets) = self.get_size_and_offset();  
        //TODO: check bounds
        self.command(lcd_commands::LCD_SETDDRAMADDR | (col + offsets[row as usize])).await;
        self.curr_row = row;
        self.curr_col = col;
    }

    pub async fn command(&self, value: u8) {
        self.send(value, 0).await;
    }

    pub async fn print(&mut self, str: &str) {
        for c in str.chars() {
            match c {
                '\n' => {
                    self.set_cursor(self.curr_row + 1, 0).await;
                }    
                _ => {
                    self.putc(c).await;
                    self.set_cursor(self.curr_row, self.curr_col + 1).await;
                }
            }
        }
    }

    pub async fn clear(&mut self) {
        self.command(lcd_commands::LCD_CLEARDISPLAY).await;
        Timer::after(Duration::from_micros(2000)).await;
        self.set_cursor(0,0).await;
    }

    pub async fn putc(&self, c: char) {
        self.send(c as u8, 1).await;
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
