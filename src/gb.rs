use alloc::boxed::Box;
use librgboy::hardware::{
    Hardware as GbHardware, Key as GbKey, SoundId, Stream, VRAM_HEIGHT, VRAM_WIDTH,
};
use log::*;
use uefi::{
    prelude::*,
    proto::console::{
        gop::{BltOp, BltPixel, GraphicsOutput},
        text::{Key, ScanCode},
    },
};
use uefi::{prelude::*, table::runtime::ResetType};

struct KeyInfo {
    key: char,
    time: u64,
}

struct Hardware {
    st: SystemTable<Boot>,
    vramsz: (usize, usize),
    vram: [u32; VRAM_HEIGHT * VRAM_WIDTH],
    pressed: Option<KeyInfo>,
}

fn tsc() -> u64 {
    #[cfg(target_arch = "x86")]
    use core::arch::x86::_rdtsc;
    #[cfg(target_arch = "x86_64")]
    use core::arch::x86_64::_rdtsc;

    unsafe { _rdtsc() as u64 }
}

impl Drop for Hardware {
    fn drop(&mut self) {
        self.clear();

        info!("Shutting down in 3 seconds...");
        self.st.boot_services().stall(3_000_000);

        let rt = self.st.runtime_services();
        rt.reset(ResetType::Shutdown, Status::SUCCESS, None);
    }
}

impl Hardware {
    fn new(st: SystemTable<Boot>) -> Self {
        Self {
            st,
            vramsz: (0, 0),
            vram: [0; VRAM_HEIGHT * VRAM_WIDTH],
            pressed: None,
        }
    }

    fn gop(&self) -> &mut GraphicsOutput {
        let gop = self
            .st
            .boot_services()
            .locate_protocol::<GraphicsOutput>()
            .expect("No graphics output protocol available");
        let gop = gop.expect("Error on opening graphics output protocol");
        unsafe { &mut *gop.get() }
    }

    fn setup(&self) {
        let mode = self
            .gop()
            .modes()
            .map(|mode| mode.expect("Couldn't get graphics mode"))
            .nth(0)
            .expect("No graphics mode");

        self.gop()
            .set_mode(&mode)
            .expect_success("Couldn't set graphics mode");

        info!("{:?}", self.gop().current_mode_info().resolution());

        self.clear();
    }

    fn clear(&self) {
        let op = BltOp::VideoFill {
            color: BltPixel::new(255, 255, 255),
            dest: (0, 0),
            dims: self.gop().current_mode_info().resolution(),
        };
        self.gop()
            .blt(op)
            .expect_success("Failed to fill screen with color");
    }

    fn set_pixel(&mut self, x: usize, y: usize, col: u32) {
        let stride = self.gop().current_mode_info().stride();
        let pixel_index = (y * stride) + x;
        let pixel_base = 4 * pixel_index;

        let r = ((col >> 16) & 0xff) as u8;
        let g = ((col >> 8) & 0xff) as u8;
        let b = (col & 0xff) as u8;

        unsafe {
            self.gop()
                .frame_buffer()
                .write_value(pixel_base, BltPixel::new(r, g, b));
        }
    }

    fn set_pixel8(&mut self, x: usize, y: usize, col: u32) {
        let (cw, ch) = (64, 32);
        let (w, h) = self.gop().current_mode_info().resolution();
        let rx = (w / cw) / 2;
        let ry = (h / ch) / 2;

        let xs = (w - cw * rx) / 2;
        let ys = (h - ch * ry) / 2;

        let xb = x * rx;
        let yb = y * ry;
        for yo in 0..ry {
            for xo in 0..rx {
                self.set_pixel(xb + xo + xs, yb + yo + ys, col);
            }
        }
    }

    fn get_key(&mut self) -> Option<Key> {
        let comp = self.st.stdin().read_key().expect("Couldn't poll key input");
        comp.expect("Couldn't extract key result")
    }
}

impl GbHardware for Hardware {
    fn joypad_pressed(&mut self, key: GbKey) -> bool {
        false
    }

    fn vram_update(&mut self, line: usize, buffer: &[u32]) {
        for x in 0..buffer.len() {
            self.set_pixel(x, line, buffer[x]);
        }
    }

    fn sound_play(&mut self, id: SoundId, stream: Box<Stream>) {}

    fn sound_stop(&mut self, id: SoundId) {}

    fn clock(&mut self) -> u64 {
        if cfg!(features = "uefi_time_source") {
            let rt = self.st.runtime_services();
            let t = rt
                .get_time()
                .expect("Couldn't get time")
                .expect("Couln't extract time");

            let days = days_from_civil(t.year() as i64, t.month() as i64, t.day() as i64);

            (days as u64) * 24 * 3600_000_000
                + (t.hour() as u64) * 3600_000_000
                + (t.minute() as u64) * 60_000_000
                + (t.second() as u64) * 1000_000
                + (t.nanosecond() / 1000) as u64
        } else {
            tsc() / 2
        }
    }

    fn send_byte(&mut self, b: u8) {}

    fn recv_byte(&mut self) -> Option<u8> {
        None
    }

    fn sched(&mut self) -> bool {
        match self.get_key() {
            Some(Key::Special(ScanCode::ESCAPE)) => return false,
            Some(Key::Printable(code)) => {
                self.pressed = Some(KeyInfo {
                    key: code.into(),
                    time: self.clock(),
                });
                debug!("pressed {}", self.pressed.as_ref().unwrap().key);
            }
            _ => {
                let clk = self.clock();

                if let Some(k) = self.pressed.as_ref() {
                    if clk.wrapping_sub(k.time) > 200_000_000 {
                        self.pressed = None;
                        debug!("released");
                    }
                }
            }
        }

        self.st.boot_services().stall(1000_000 / 600);

        true
    }
}

#[allow(unused)]
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400);
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub fn run(st: SystemTable<Boot>) -> ! {
    let hw = Hardware::new(st);

    hw.setup();

    librgboy::run(
        librgboy::Config::new().native_speed(true),
        include_bytes!("roms/zelda.gb").to_vec(),
        hw,
    );

    loop {}
}
