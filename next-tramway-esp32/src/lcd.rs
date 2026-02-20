use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use embassy_time::{Timer, Duration};
use esp_hal::{Blocking, i2c::master::I2c};
use heapless::String;

use crate::display::{TramDirectionState, TramDisplay};
use core::fmt::Write;

// add space padding at the end of the string to ensure that when we update the LCD, we properly clear the previous content if the new one is shorter
fn pad_to_width<const N: usize>(
    s: &mut heapless::String<N>,
    width: usize,
) {
    let len = s.len();

    if len < width {
        for _ in 0..(width - len) {
            let _ = s.push(' ');
        }
    }
}

// simple text wrapper that adds newlines to fit the text within the given width
pub fn wrap_text<const OUT: usize>(
    input: &str,
    line_width: usize,
    output: &mut String<OUT>,
) {
    output.clear();

    let mut current_width = 0;

    for c in input.chars() {
        if current_width >= line_width {
            if output.push('\n').is_err() {
                return; // overflow 
            }
            current_width = 0;
        }

        if output.push(c).is_err() {
            return; // overlow
        }

        current_width += 1;
    }
}

pub struct LcdRenderer<'a> {
    lcd_screen: Lcd<'a>, // handle to the LCD screen, used to send commands and data to the LCD
    last_rendered: Option<TramDirectionState>, // we keep track of the last rendered state to avoid unnecessary updates to the LCD, which can be slow (especially over I2C)
    last_rendered_line: Option<heapless::String<16>>, 
    display_buffer: [heapless::String<20>; 4], // we keep a buffer of the currently displayed content on the LCD to minimize the number of updates, which is slow 
}

impl<'a> LcdRenderer<'a> {
    pub fn new(lcd_screen: Lcd<'a>) -> Self {
        LcdRenderer { 
            lcd_screen,
            last_rendered: None,
            last_rendered_line: None,
            display_buffer: [
                heapless::String::new(),
                heapless::String::new(),
                heapless::String::new(),
                heapless::String::new(),
             ]
        }
    }


    async fn render_line(&mut self, line: &heapless::String<16>,tram_direction_state: &TramDirectionState) {
        if self.last_rendered.as_ref() == Some(tram_direction_state) 
          && self.last_rendered_line.as_ref() == Some(line) {
            // technically the display buffer would also be the same
            // but it skips the whole rendering logic at the expense of some memory 

            return; // nothing changed
        }
        let mut new_buffer: [heapless::String<20>; 4] = Default::default();
        let _ = new_buffer[0].push_str(line);

        if tram_direction_state.next_passages.is_empty() {
            let _ = new_buffer[1].push_str("Pas de passage dans");
            let _ = new_buffer[2].push_str("l'heure...");
        } else {
            let mut buf: heapless::String<20> = heapless::String::new();
            for (i, next) in tram_direction_state.next_passages.iter().enumerate() {
                buf.clear();
                let _ = write!(new_buffer[i + 1], "{:<17} {:>2}", next.destination, next.relative_arrival);
            }
        }
        let _ = write!(
            new_buffer[3],
            "{:>20}",
            tram_direction_state.update_at
        );

        self.last_rendered = Some(tram_direction_state.clone());
        self.last_rendered_line = Some(line.clone());

        // the true bottleneck is the LCD update
        // trading CPU for less I2C traffic is worth it
        let (_, width, _) = self.lcd_screen.get_size_and_offset();

        for i in 0..4 {
            pad_to_width(&mut new_buffer[i], (width +1) as usize);
        }

        for i in 0..4 {
            if self.display_buffer[i] != new_buffer[i] {
                self.lcd_screen.set_cursor(i as u8, 0).await;
                self.lcd_screen.print(&new_buffer[i]).await;
                self.display_buffer[i] = new_buffer[i].clone();
            }
        }
    }
}

// assume a 20x04 LCD screen is used
// I feel like 16x02 would be too small anyway
impl TramDisplay for LcdRenderer<'_> {
    async fn render<'b>(&'b mut self, state: &'b crate::display::UiState) {
        if state.lines.is_empty() {
            if let Some(message) = &state.current_message {
                self.lcd_screen.clear().await;
                let (_, line_width, _) = self.lcd_screen.get_size_and_offset();
                let mut buffer: heapless::String<80> = heapless::String::new();
                wrap_text(message, (line_width + 1) as usize, &mut buffer);
                // esp_println::println!("{}",buffer);
                self.lcd_screen.print(&buffer).await;
            }
            return;
        }

        let Some(line) = state.lines.get(state.current_line) else { return };
        if let Some(directions) = line.directions.get(state.current_direction_id) {
            self.render_line(&line.line,directions).await;
        }
    }
}

// could be more generic, but this is good enough for our use case, and we can always refactor later if needed
pub enum LcdGeometry {
    L1602, // 16 characters, 2 lines
    L2004, // 20 characters, 4 lines
}

mod lcd_bits {
    pub const EN: u8 = 0b0000_0100; 
    // pub const RW: u8 = 0b0000_0010;
    // pub const RS: u8 = 0b0000_0001;
    pub const BL: u8 = 0b0000_1000;
}

mod lcd_commands {
    pub const LCD_SETDDRAMADDR: u8 = 0x80;
    pub const LCD_CLEARDISPLAY: u8 = 0x01;
}

pub struct Lcd<'a> {
    bus: &'a Mutex<CriticalSectionRawMutex, Option<I2c<'static, Blocking>>>,
    i2c_addr: u8,
    geom: LcdGeometry,
    curr_row: u8,
    curr_col: u8
}

// Source: https://cdn.sparkfun.com/assets/9/5/f/7/b/HD44780.pdf
// assumes that the LCD is connected in 4-bit mode, with an I2C backpack (e.g. based on PCF8574) that maps the I2C data to the LCD pins as follows:
// D7 D6 D5 D4 BL EN RW RS
// some things could be enhanced here in the future, probably
// Doesn't contain the rendering logic, just the low-level commands to control the LCD (used by the LcdRenderer to render the UI state)
impl<'a> Lcd<'a> {

    pub fn new(
        bus: &'a Mutex<CriticalSectionRawMutex, Option<I2c<'static, Blocking>>>,
        i2c_addr: u8,
        geom: LcdGeometry
    ) -> Self {
        Self { i2c_addr, bus, geom, curr_row: 0, curr_col: 0 }
    }

    // set the LCD in the desired mode and initialize it, needs to be called before any other command
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
            LcdGeometry::L1602 => (1, 15, &[0x00, 0x40]),
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

    async fn command(&self, value: u8) {
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

    async fn send(&self, value: u8, mode: u8) {
        let highnib = value & 0xF0;
        let lownib = (value << 4) & 0xF0;
        self.write_4_bits(highnib | mode | lcd_bits::BL).await;
        self.write_4_bits(lownib | mode | lcd_bits::BL).await;
    }

    // D7 D6 D5 D4 BL EN RW RS
    async fn write_4_bits(&self, value: u8) {
        let mut guard = self.bus.lock().await;
        let i2c = guard.as_mut().expect("I2C not initialized");
        self.write_i2c(i2c, value);
        self.pulse_enable(i2c, value).await;
    }

    fn write_i2c(&self, i2c_bus: &mut I2c<'_, Blocking>, data: u8) {
        let result = i2c_bus.write(self.i2c_addr, &[data]);

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

    async fn set_4_bits_mode(&self) {
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
