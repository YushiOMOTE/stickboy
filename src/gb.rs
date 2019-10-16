use alloc::{boxed::Box, vec, vec::Vec};
use log::*;
use rgy::hardware::{Hardware as GbHardware, Key as GbKey, Stream, VRAM_HEIGHT, VRAM_WIDTH};
use uefi::{
    prelude::*,
    proto::console::{
        gop::{BltOp, BltPixel, BltRegion, GraphicsOutput},
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
    vramlast: u64,
    vramscale: usize,
    keylast: u64,
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

fn pix(col: u32) -> BltPixel {
    let r = (col >> 16) as u8;
    let g = (col >> 8) as u8;
    let b = col as u8;
    BltPixel::new(r, g, b)
}

impl Hardware {
    fn new(st: SystemTable<Boot>) -> Self {
        Self {
            st,
            vramsz: (0, 0),
            vram: [0; VRAM_HEIGHT * VRAM_WIDTH],
            vramlast: 0,
            vramscale: 1,
            keylast: 0,
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

    fn setup(&mut self) {
        let mode = self
            .gop()
            .modes()
            .map(|mode| mode.expect("Couldn't get graphics mode"))
            // .find(|ref mode| {
            //     let info = mode.info();
            //     info.resolution() == (1024, 768)
            // })
            .nth(0)
            .expect("No graphics mode");

        self.gop()
            .set_mode(&mode)
            .expect_success("Couldn't set graphics mode");

        info!("{:?}", self.gop().current_mode_info().resolution());

        let xscale = self.gop().current_mode_info().resolution().0 / VRAM_WIDTH;
        let yscale = self.gop().current_mode_info().resolution().1 / VRAM_HEIGHT;
        self.vramscale = xscale.min(yscale).max(1);

        self.clear();
    }

    fn update_vram(&self) {
        let scale = 1; //self.vramscale;

        let w = VRAM_WIDTH;
        let h = VRAM_HEIGHT;

        for vramy in 0..scale {
            for vramx in 0..scale {
                let xbase = vramx * w;
                let ybase = vramy * h;

                let subvram: Vec<_> = (0..(w * h))
                    .map(|i| {
                        let x = ((i % w) + xbase) / scale;
                        let y = ((i / w) + ybase) / scale;
                        pix(self.vram[y * w + x])
                    })
                    .chain((0..w).map(|_| pix(0)))
                    .collect();

                let op = BltOp::BufferToVideo {
                    buffer: &subvram,
                    src: BltRegion::Full,
                    dest: (xbase, ybase),
                    dims: (w, h),
                };
                self.gop()
                    .blt(op)
                    .expect_success("Failed to fill screen with color");
            }
        }
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
            self.vram[VRAM_WIDTH * line + x] = buffer[x];
        }
    }

    fn sound_play(&mut self, stream: Box<dyn Stream>) {}

    fn load_ram(&mut self, size: usize) -> Vec<u8> {
        vec![9; size]
    }

    fn save_ram(&mut self, ram: &[u8]) {}

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
            tsc() / 1000
        }
    }

    fn send_byte(&mut self, b: u8) {}

    fn recv_byte(&mut self) -> Option<u8> {
        None
    }

    fn sched(&mut self) -> bool {
        if self.clock() - self.keylast >= 20_000 {
            self.keylast = self.clock();

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
        }

        if self.clock() - self.vramlast >= 50_000 {
            self.vramlast = self.clock();
            self.update_vram();
        }

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
    let mut hw = Hardware::new(st);

    hw.setup();

    rgy::run(
        rgy::Config::new().native_speed(true),
        include_bytes!("roms/zelda.gb").to_vec(),
        hw,
    );

    loop {}
}
