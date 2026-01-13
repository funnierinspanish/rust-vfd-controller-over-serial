use serialport::{DataBits, FlowControl, Parity, SerialPort, StopBits};
use std::io::{self, Write};
use std::thread::sleep;
use std::time::Duration;

const CMD_CLEAR: u8 = 0x0C;
const CMD_ESC: u8 = 0x1B;
const CMD_US: u8 = 0x1F;

enum TextFit {
    OneLine,
    NeedsWrap,
    TooLong,
    OneLineTruncated,
}

struct BirchVfd {
    port: Box<dyn SerialPort>,
    width: u8,
    height: u8,
    cursor_x: u8,
    cursor_y: u8,
}

impl BirchVfd {
    pub fn new(
        device_path: &str,
        width: u8,
        height: u8,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let port = serialport::new(device_path, 9600)
            .data_bits(DataBits::Eight)
            .flow_control(FlowControl::None)
            .parity(Parity::None)
            .stop_bits(StopBits::One)
            .timeout(Duration::from_millis(1000))
            .open()?;

        let mut vfd = BirchVfd {
            port,
            width,
            height,
            cursor_x: 1,
            cursor_y: 1,
        };
        vfd.initialize()?;
        Ok(vfd)
    }

    // Send the standard initialization command (ESC @)
    fn initialize(&mut self) -> Result<(), io::Error> {
        // ESC @ = Initialize display
        let cmd = [CMD_ESC, 0x40];
        self.port.write_all(&cmd)?;
        Ok(())
    }

    // Clear screen and return cursor to home (top-left)
    pub fn clear(&mut self) -> Result<(), io::Error> {
        self.port.write_all(&[CMD_CLEAR])?;
        // VFDs are slow; a tiny flush ensures the command hits the hardware
        match self.port.flush() {
            Ok(_) => (),
            Err(e) => eprintln!(
                "Warning: Failed to flush Serial port after clear command: {}",
                e
            ),
        }
        self.set_cursor(0, 0).expect("Failed to position cursor");
        Ok(())
    }

    // Move cursor to specific column (x) and row (y) (1-indexed)
    pub fn set_cursor(&mut self, x: u8, y: u8) -> Result<(), io::Error> {
        // Make sure the cursor stays within bounds
        self.cursor_x = if x > self.width { self.width } else { x };
        self.cursor_y = if y > self.height { self.height } else { y };
        let cmd = [CMD_US, "$".as_bytes()[0], x + 1, y + 1];
        self.port.write_all(&cmd)?;
        Ok(())
    }

    pub fn get_cursor(&self) -> (u8, u8) {
        (self.cursor_x, self.cursor_y)
    }

    fn write(&mut self, text: &str) -> Result<(), io::Error> {
        self.port
            .write_all(text.as_bytes())
            .expect("Failed to write to serial port.");
        Ok(())
    }

    // Write a single line to the display
    pub fn writeln(&mut self, text: &str) -> Result<(), io::Error> {
        self.write(text).expect("Failed to write line");
        Ok(())
    }

    // Write a single line to the display and truncate if necessary
    pub fn writeln_truncate(&mut self, text: &str) -> Result<(), io::Error> {
        let space_available = self.get_space_available_on_line();
        let truncated_text = &text.as_bytes()[..space_available];
        let truncated_str = String::from_utf8_lossy(truncated_text).to_string();

        self.write(&truncated_str)
            .expect("Failed to write truncated line");
        Ok(())
    }

    fn write_multi_line(&mut self, text: &str) -> Result<(), io::Error> {
        let mut remaining_bytes = text.as_bytes();
        while !remaining_bytes.is_empty() {
            let (cursor_x, cursor_y) = self.get_cursor();
            let space_available = (self.width - cursor_x) as usize;
            let bytes_to_take = space_available.min(remaining_bytes.len());
            let chunk = String::from_utf8_lossy(&remaining_bytes[..bytes_to_take])
                .trim()
                .to_string();

            self.write(&chunk).expect("Failed to write chunk");
            remaining_bytes = &remaining_bytes[bytes_to_take..];

            if remaining_bytes.is_empty() {
                break;
            } else {
                self.set_cursor(0, cursor_y + 1)
                    .expect("Failed to set cursor for wrap_line");
            }
        }

        Ok(())
    }

    fn get_space_available_on_line(&self) -> usize {
        let (cursor_x, _) = self.get_cursor();
        (self.width - cursor_x) as usize
    }

    fn get_lines_available(&self) -> usize {
        let (_, cursor_y) = self.get_cursor();
        (self.height - (cursor_y + 1)) as usize
    }

    pub fn write_text(&mut self, text: &str) -> Result<(), io::Error> {
        self.write_text_handler(text, false)
    }

    pub fn write_text_truncate(&mut self, text: &str) -> Result<(), io::Error> {
        self.write_text_handler(text, true)
    }

    fn write_text_handler(&mut self, text: &str, truncate: bool) -> Result<(), io::Error> {
        // Check if the text would fit
        let space_left_on_line = self.get_space_available_on_line();

        match self.get_text_fit(text, truncate) {
            TextFit::OneLine => {
                self.writeln(text).expect("Failed to write line");
            }
            TextFit::OneLineTruncated => {
                self.writeln_truncate(text).expect("Failed to write line");
            }
            TextFit::NeedsWrap => {
                self.write_multi_line(text)
                    .expect("Failed to write multi line");
            }
            TextFit::TooLong => {
                return Err(io::Error::other(
                    format!(
                        "Text too long to fit on display. A maximum of {} characters are available from the current cursor position: {}, {}. {} characters were provided.",
                        space_left_on_line * self.get_lines_available(),
                        self.get_cursor().0,
                        self.get_cursor().1,
                        text.len()
                    ),
                ));
            }
        }

        Ok(())
    }

    // Determine if the text fits on the display and how to handle it
    //  based on the current cursor position, display size,
    //  and user preferences for wrapping and truncation.
    fn get_text_fit(&self, text: &str, truncate: bool) -> TextFit {
        let bytes = text.as_bytes();
        let text_length = bytes.len() as u8;

        let (cursor_x, cursor_y) = self.get_cursor();
        let space_left_on_line = self.width - (cursor_x);
        let lines_left = self.height - (cursor_y + 1);

        if text_length <= self.width {
            return TextFit::OneLine;
        }

        if cursor_x < self.width && truncate {
            return TextFit::OneLineTruncated;
        }

        // Text is longer than one line, but still would fit if wrapped
        if space_left_on_line + (lines_left * self.width) >= text_length {
            TextFit::NeedsWrap
        } else {
            TextFit::TooLong
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut vfd = BirchVfd::new("/dev/ttyUSB0", 20, 2).expect("Failed to connect to device.");

    println!("Device connected. Sending data...");

    vfd.clear().expect("Failed to clear display");

    vfd.set_cursor(0, 0).expect("Failed to position cursor");

    vfd.writeln("Epale!").expect("Failed to write to display");

    sleep(Duration::from_secs(1));

    vfd.set_cursor(7, 0).expect("Failed to position cursor");

    sleep(Duration::from_secs(2));
    vfd.write_text(":) yuju!")
        .expect("Failed to write to display");
    sleep(Duration::from_secs(2));

    vfd.clear().expect("Failed to clear display");

    vfd.write_text("Rust speaking serial to a *VFD* :)")
        .expect("Failed to write to display");

    Ok(())
}
