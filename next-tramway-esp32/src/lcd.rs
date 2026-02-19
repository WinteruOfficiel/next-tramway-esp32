use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use embassy_time::{Timer, Duration};
use esp_hal::{Blocking, i2c::master::I2c};

use crate::display::{TramDirectionState, TramDisplay, UiState};
use core::fmt::Write;

pub struct LcdRenderer<'a> {
    lcd_screen: Lcd<'a>,
    last_rendered: Option<TramDirectionState>,
    last_rendered_line: Option<heapless::String<16>>
}

impl<'a> LcdRenderer<'a> {
    pub fn new(lcd_screen: Lcd<'a>) -> Self {
        LcdRenderer { 
            lcd_screen,
            last_rendered: None,
            last_rendered_line: None,
        }
    }


    async fn render_line(&mut self, line: &heapless::String<16>,tram_direction_state: &TramDirectionState) {
        if self.last_rendered.as_ref() == Some(tram_direction_state) 
          && self.last_rendered_line.as_ref() == Some(line) {
            return; // nothing changed
        }
        self.lcd_screen.clear().await;
        self.lcd_screen.print(line).await;
        self.lcd_screen.set_cursor(1, 0).await;

        if tram_direction_state.next_passages.is_empty() {
            self.lcd_screen.print("Pas de passage...").await;
        } else {
            let mut buf: heapless::String<20> = heapless::String::new();
            for (i, next) in tram_direction_state.next_passages.iter().enumerate() {
                buf.clear();
                let _ = write!(buf, "{:<17} {:>2}", next.destination, next.relative_arrival);
                self.lcd_screen.set_cursor(i as u8 + 1, 0).await;
                self.lcd_screen.print(&buf).await;
            }
            self.lcd_screen.set_cursor(3, 12).await;
            self.lcd_screen.print(&tram_direction_state.update_at).await;

            self.last_rendered = Some(tram_direction_state.clone());
            self.last_rendered_line = Some(line.clone());
        }
    }
}

impl TramDisplay for LcdRenderer<'_> {
    async fn render<'b>(&'b mut self, state: &'b crate::display::UiState) {
        if state.lines.is_empty() {
            return;
        }

        let Some(line) = state.lines.get(state.current_line) else { return };
        if let Some(directions) = line.directions.get(state.current_direction_id) {
            self.render_line(&line.line,directions).await;
        }
    }
}

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
        geom: LcdGeometry
    ) -> Self {
        Self { bus, geom, curr_row: 0, curr_col: 0 }
    }

    pub async fn init(&self) {
        self.set_4_bits_mode().await;
        Timer::after(Duration::from_millis(5)).await;

        self.send(0x28, 0).await; // 4-bit, 2-line
        self.send(0x08, 0).await; // display OFF
        self.send(0x01, 0).await; // clear
        Timer::after(Duration::from_millis(2)).await;
        self.send(0x06, 0).await; // entry mode
        self.send(0x0C, 0).await; // display ON
    }

    fn get_size_and_offset(&self) -> (u8, u8, &[u8]) {
        match self.geom {
            LcdGeometry::L1602 => (1, 15, &[0x00, 0x40][..]),
            LcdGeometry::L2004 => (3, 19, &[0x00, 0x40, 0x14, 0x54]),
        }
    }

    pub async fn set_cursor(&mut self, row:  u8, col: u8) {
        let (_max_row, _max_col, offsets) = self.get_size_and_offset();  
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
        Timer::after(Duration::from_millis(1)).await;
    }
}
