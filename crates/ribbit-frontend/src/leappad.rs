//! Playing the original LeapPad (`ribbit_leappad`): the machine runs on its
//! own thread in real time; the window draws the pad with the open virtual
//! book on it and turns the mouse into the pen. Everything the pad does in
//! answer comes from the cartridge's own software, through the emulated pen
//! sensor (docs/leappad.md). The EEPROM (the pen calibration and what the
//! firmware keeps there) is kept in the save folder.
//!
//! Sound: the sound processor's 8 kHz mix (ribbit_leappad::sound) goes to
//! the same output queue as the Leapster's.

use crate::audio_out::AudioSink;
use ribbit_leappad::book::{self, Book, PAGE_H, PAGE_W, Problem, Side, Spot, TouchReport};
use ribbit_leappad::{MACHINE_HZ, Machine};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Ribbit's own test book (an original fixture: numbered targets, no
/// pictures), always available from the menu.
pub const TEST_BOOK: &str = include_str!("../../ribbit-leappad/books/pen-test.toml");

/// Lines of the firmware's debug log kept for the panel.
const LOG_LINES: usize = 400;

/// What the panel asks the app to do.
pub enum Event {
    PickBook,
    TestBook,
    CloseBook,
    /// The book drawn from the cartridge's own layout.
    CartridgeBook,
}

struct Shared {
    m: Mutex<Machine>,
    quit: AtomicBool,
    /// Paused (or in the background): the machine waits.
    paused: AtomicBool,
    /// The breakpoint the machine stopped at (debugger), for the app.
    hit: Mutex<Option<u16>>,
    pen: Mutex<Option<Spot>>,
    power: AtomicBool,
    off: AtomicBool,
    log: Mutex<VecDeque<String>>,
    touches: Mutex<VecDeque<(Instant, TouchReport)>>,
    /// Percent of real time over the last second (f32 bits).
    speed: AtomicU32,
    /// Volume, as f32 bits.
    gain: AtomicU32,
    sink: Mutex<Option<AudioSink>>,
}

pub struct LeapPadSession {
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// The book's name ("Tutter's Tiny Trip (USA)").
    pub title: String,
    /// Its series, when the name gives one ("Leap Start Pre Reading", "Level 3").
    pub series: Option<String>,
    /// When it was started (Unix seconds, for Discord).
    pub started: u64,
    /// The cartridge file (None: the pad's own built-in book).
    pub cart: Option<PathBuf>,
    eeprom_path: PathBuf,
    pub book: Option<Book>,
    pub book_problems: Vec<Problem>,
    /// Why no book could be drawn from the cartridge, if it could not.
    pub layout_note: Option<String>,
    /// The cartridge file is a damaged dump whose checksum was made to match
    /// (ribbit_leappad::repair_checksum), so the pad plays it.
    pub repaired: bool,
    /// The base ROM, cartridge and EEPROM, for the words job (a second pad).
    words_input: (Vec<u8>, Option<Vec<u8>>, Option<Vec<u8>>),
    /// The words job (leappad_words.rs), while it runs.
    #[cfg(not(target_os = "android"))]
    words_job: Option<crate::leappad_words::Job>,
    /// The speech model's download, while it runs.
    #[cfg(not(target_os = "android"))]
    words_download: Option<crate::leappad_words::Download>,
    /// What the words are doing, for the bar (empty: nothing to say).
    pub words_status: String,
    /// The data folder (the model and the kept words), once words are wanted.
    words_data: Option<PathBuf>,
    pub spread: usize,
    pictures: HashMap<PathBuf, Option<egui::TextureHandle>>,
    /// Where the pen is (shown) and whether it is down.
    pen: Option<(Spot, bool)>,
    /// Scripted touches (RIBBIT_LEAPPAD_SCRIPT, and the automatic GO
    /// touches): the time they started and what is left.
    script: Option<(Instant, VecDeque<ScriptTouch>)>,
    /// The pen touches GO by itself (settings `leappad_auto_go`).
    pub auto_go: bool,
}

/// One scripted touch: at `at` seconds, show spread `spread` and touch its
/// GO (`region` None) or the middle of its region `region`.
struct ScriptTouch {
    at: f32,
    spread: usize,
    region: Option<usize>,
}

/// How long a scripted pen hovers before it presses, and how long it stays down.
const SCRIPT_HOVER: f32 = 0.6;
const SCRIPT_DOWN: f32 = 0.3;

/// Reads a touch script: one touch per line, `<seconds> go <spread>` or
/// `<seconds> spot <spread> <region>` (spreads and regions counted from 0 in
/// the open book); `#` starts a comment.
fn read_script(path: &Path) -> Option<VecDeque<ScriptTouch>> {
    let text = std::fs::read_to_string(path).map_err(|e| tracing::warn!(target: "ribbit::leappad", "touch script {}: {e}", path.display())).ok()?;
    let mut out = VecDeque::new();
    for line in text.lines() {
        let w: Vec<&str> = line.split('#').next().unwrap_or("").split_whitespace().collect();
        let touch = match w.as_slice() {
            [t, "go", sp] => t.parse().ok().zip(sp.parse().ok()).map(|(at, spread)| ScriptTouch { at, spread, region: None }),
            [t, "spot", sp, r] => (|| Some(ScriptTouch { at: t.parse().ok()?, spread: sp.parse().ok()?, region: Some(r.parse().ok()?) }))(),
            [] => continue,
            _ => None,
        };
        match touch {
            Some(t) => out.push_back(t),
            None => tracing::warn!(target: "ribbit::leappad", "touch script: can't read {line:?}"),
        }
    }
    Some(out)
}

/// A cartridge file's name without the archive's "(LeapPad, 500-00652) "
/// prefix, for the window title and Discord: "Leap 1 Reading - Disney's
/// Bounce, Tigger, Bounce (USA)".
pub fn book_title(stem: &str) -> String {
    let t = stem.trim();
    match t.strip_prefix('(').and_then(|r| r.split_once(')')) {
        Some((inside, rest)) if inside.contains("LeapPad") && !rest.trim().is_empty() => rest.trim().to_string(),
        _ => t.to_string(),
    }
}

/// A book name's series and title: "Level 3 - The Bike Race (USA)" ->
/// (Some("Level 3"), "The Bike Race (USA)"); a name without " - " has none.
pub fn book_series(name: &str) -> (Option<String>, String) {
    match name.split_once(" - ") {
        Some((series, title)) if !series.trim().is_empty() && !title.trim().is_empty() => (Some(series.trim().to_string()), title.trim().to_string()),
        _ => (None, name.to_string()),
    }
}

/// The cartridge part number in a file name, e.g. "500-00406".
pub fn part_number(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    let b = name.as_bytes();
    (0..b.len().saturating_sub(8)).find_map(|i| {
        let w = &b[i..i + 9];
        (w[..3].iter().all(u8::is_ascii_digit) && w[3] == b'-' && w[4..].iter().all(u8::is_ascii_digit))
            .then(|| name[i..i + 9].to_string())
    })
}

impl LeapPadSession {
    /// Powers on the pad with its base ROM and a cartridge (or none).
    /// `classic`: play LeapPad Classic cartridges (Bus::classic).
    pub fn start(bios: &Path, cart: Option<&Path>, save_dir: &Path, sink: Option<AudioSink>, classic: bool) -> anyhow::Result<Self> {
        use anyhow::Context;
        let rom = std::fs::read(bios).with_context(|| format!("reading {}", bios.display()))?;
        if ribbit_leappad::identify(&rom) != Some(ribbit_leappad::Image::BaseRom) {
            anyhow::bail!("{} is not a LeapPad base ROM", bios.display());
        }
        let mut m = Machine::new(rom.clone());
        m.bus.classic = classic;
        let mut cart_data = None;
        let mut repaired = false;
        if let Some(c) = cart {
            let mut data = std::fs::read(c).with_context(|| format!("reading {}", c.display()))?;
            if ribbit_leappad::identify(&data) != Some(ribbit_leappad::Image::Cartridge) {
                anyhow::bail!("{} is not a LeapPad cartridge", c.display());
            }
            if let Some(line) = ribbit_leappad::lost_address_line(&data) {
                anyhow::bail!(
                    "{} is a damaged dump: address line A{line} was not read when it was made, so every other block of {} bytes is a copy of the one before and half of the cartridge is missing. A LeapPad refuses it (\"Bad Cart\"). A good dump of this cartridge is needed.",
                    c.display(),
                    1u32 << line
                );
            }
            if let Some(sum) = ribbit_leappad::repair_checksum(&mut data) {
                tracing::warn!(target: "ribbit::leappad", "{}: checksum {:04X}, the file's own says {:04X}: a damaged dump; played anyway", c.display(), sum.calc, sum.stored);
                repaired = true;
            }
            cart_data = Some(data.clone());
            m.insert_cartridge(data);
        }
        let eeprom_path = save_dir.join("leappad.eeprom");
        let eeprom = std::fs::read(&eeprom_path).ok();
        if let Some(e) = &eeprom {
            m.set_eeprom(e.clone());
        }
        let shared = Arc::new(Shared {
            m: Mutex::new(m),
            quit: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            hit: Mutex::new(None),
            pen: Mutex::new(None),
            power: AtomicBool::new(false),
            off: AtomicBool::new(false),
            log: Mutex::new(VecDeque::new()),
            touches: Mutex::new(VecDeque::new()),
            speed: AtomicU32::new(0),
            gain: AtomicU32::new(1.0f32.to_bits()),
            sink: Mutex::new(sink),
        });
        let s = shared.clone();
        // Above normal, on the fast cores, as the other consoles' threads: at
        // normal priority, on a busy 4-thread PC it waited behind other
        // programs for up to 130 ms at a time and its sound clicked.
        let thread = std::thread::Builder::new().name("leappad".into()).spawn(move || {
            crate::perf::raise_thread_priority();
            run(&s)
        })?;
        let (series, title) = match cart {
            Some(c) => c.file_stem().map_or_else(|| (None, "LeapPad".into()), |s| book_series(&book_title(&s.to_string_lossy()))),
            None => (None, "LeapPad (built-in book)".into()),
        };
        Ok(LeapPadSession {
            shared,
            thread: Some(thread),
            title,
            series,
            started: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()),
            cart: cart.map(Path::to_path_buf),
            eeprom_path,
            book: None,
            book_problems: Vec::new(),
            layout_note: None,
            repaired,
            words_input: (rom, cart_data, eeprom),
            #[cfg(not(target_os = "android"))]
            words_job: None,
            #[cfg(not(target_os = "android"))]
            words_download: None,
            words_status: String::new(),
            words_data: None,
            spread: 0,
            pictures: HashMap::new(),
            pen: None,
            script: std::env::var_os("RIBBIT_LEAPPAD_SCRIPT").and_then(|p| read_script(Path::new(&p))).map(|s| (Instant::now(), s)),
            auto_go: true,
        })
    }

    /// The key the book is remembered under in the settings.
    pub fn book_key(&self) -> String {
        self.cart.as_ref().map_or_else(String::new, |c| c.display().to_string())
    }

    /// Opens a virtual book; returns what was wrong with it (errors mean it
    /// was not opened).
    pub fn open_book(&mut self, loaded: Result<(Book, Vec<Problem>), Vec<Problem>>) -> Vec<Problem> {
        match loaded {
            Ok((b, mut problems)) => {
                if let (Some(cart), Some(want)) = (&self.cart, &b.cartridge)
                    && part_number(cart).is_some_and(|p| !p.eq_ignore_ascii_case(want))
                {
                    problems.push(Problem {
                        error: false,
                        message: format!("this book is for cartridge {want}, not the one in the pad"),
                    });
                }
                self.book = Some(b);
                self.spread = 0;
                self.pictures.clear();
                self.book_problems = problems.clone();
                problems
            }
            Err(problems) => problems,
        }
    }

    /// Opens the book Ribbit draws from the cartridge's own layout (the
    /// outlines of everything touchable, and the GO spots). Returns whether
    /// the cartridge's layout could be read.
    pub fn open_cartridge_book(&mut self) -> bool {
        let Some(cart) = self.cart.clone() else { return false };
        let built = std::fs::read(&cart)
            .map_err(|e| e.to_string())
            .and_then(|d| Book::from_cartridge(&d, &self.title, part_number(&cart).as_deref()));
        match built {
            Ok(b) => {
                self.layout_note = None;
                self.open_book(Ok((b, Vec::new())));
                true
            }
            Err(e) => {
                self.layout_note = Some(e);
                false
            }
        }
    }

    /// The pen touches the first page's GO once the pad has started (4 s
    /// after power on: the cartridge's start-up tune is done and it listens
    /// for the pen; lpwords' touches start then too).
    pub fn go_first_page(&mut self) {
        let (started, touches) = self.script.get_or_insert_with(|| (Instant::now(), VecDeque::new()));
        let at = started.elapsed().as_secs_f32() + 4.0;
        touches.push_back(ScriptTouch { at, spread: 0, region: None });
    }

    /// Shows the book's words: those kept from before, and the rest made in
    /// the background (leappad_words.rs) if the speech model is here. `on`:
    /// the setting (off: the book's boxes stay empty).
    pub fn start_words(&mut self, data: &Path, on: bool) {
        self.words_data = Some(data.to_path_buf());
        #[cfg(not(target_os = "android"))]
        {
            use crate::leappad_words as w;
            self.words_job = None;
            self.words_status.clear();
            let Some(cart) = self.words_input.1.clone() else { return };
            if !on || !self.book.as_ref().is_some_and(|b| b.from_cartridge) {
                return;
            }
            let (lines, done) = w::read_cache(&w::cache_path(data, &cart));
            if let Some(b) = &mut self.book {
                for (spread, text) in &lines {
                    b.clear_words(*spread);
                    b.apply_words(text);
                }
                if done.len() >= b.spreads.len() {
                    return;
                }
            }
            if !w::model_ready(data) {
                self.words_status = "The book's words need the speech model".into();
                return;
            }
            let (bios, eeprom) = (self.words_input.0.clone(), self.words_input.2.clone());
            self.words_job = Some(w::start(bios, cart, eeprom, data.to_path_buf()));
        }
        #[cfg(target_os = "android")]
        let _ = on;
    }

    /// Takes the words job's and the download's news (each frame).
    fn poll_words(&mut self, ctx: &egui::Context) {
        #[cfg(not(target_os = "android"))]
        {
            use crate::leappad_words as w;
            if let Some(d) = &self.words_download {
                match d.result.try_recv() {
                    Ok(Ok(())) => {
                        self.words_download = None;
                        if let Some(data) = self.words_data.clone() {
                            self.start_words(&data, true);
                        }
                    }
                    Ok(Err(e)) => {
                        self.words_download = None;
                        self.words_status = e;
                    }
                    Err(_) => {
                        let mb = d.progress.load(Ordering::Relaxed) >> 20;
                        self.words_status = format!("Downloading the speech model: {mb} of {} MB…", w::MODEL_MB);
                        ctx.request_repaint_after(Duration::from_millis(250));
                    }
                }
            }
            let Some(job) = &self.words_job else { return };
            job.focus.store(self.spread, Ordering::Relaxed);
            let mut finished = false;
            while let Ok(msg) = job.rx.try_recv() {
                match msg {
                    w::Msg::Spread(spread, text) => {
                        if let Some(b) = &mut self.book {
                            b.clear_words(spread);
                            b.apply_words(&text);
                        }
                    }
                    w::Msg::Status(s) => self.words_status = s,
                    w::Msg::Failed(e) => {
                        tracing::warn!(target: "ribbit::leappad", "words: {e}");
                        self.words_status = format!("Couldn't load the words: {e}");
                        finished = true;
                    }
                    w::Msg::Done => {
                        self.words_status.clear();
                        finished = true;
                    }
                }
            }
            if finished {
                self.words_job = None;
            } else {
                ctx.request_repaint_after(Duration::from_millis(500));
            }
        }
        #[cfg(target_os = "android")]
        let _ = ctx;
    }

    /// The breakpoint the pad stopped at (once).
    pub fn take_breakpoint(&self) -> Option<u16> {
        self.shared.hit.lock().unwrap().take()
    }

    /// What the debugger shows (debugger.rs): the 8051's memories at their
    /// own bases (the ROMs at their physical addresses, on-chip XDATA at
    /// 0x2000_F000, internal RAM at 0x3000_0000, the SFRs at 0x3100_0080),
    /// its registers and the code from the PC.
    pub fn debug_snapshot(&self, dbg: &mut crate::debugger::DebugUi, breakpoints: &[u32], paused: bool) -> crate::debugger::Snapshot {
        use crate::debugger::{ROWS, Region, Snapshot};
        let m = self.shared.m.lock().unwrap_or_else(|e| e.into_inner());
        let mut regions = vec![Region { name: "Base ROM", base: 0, len: m.bus.bios.len() as u32, writable: false }];
        if let Some(c) = &m.bus.cart {
            regions.push(Region { name: "Cartridge", base: ribbit_leappad::bus::CART_BASE, len: c.len() as u32, writable: false });
        }
        regions.push(Region { name: "On-chip RAM (XDATA F000)", base: 0x2000_F000, len: 0x1000, writable: true });
        regions.push(Region { name: "Internal RAM", base: 0x3000_0000, len: 0x100, writable: true });
        regions.push(Region { name: "SFRs (80-FF)", base: 0x3100_0080, len: 0x80, writable: true });
        let top = dbg.view(&regions);
        let bytes = (0..16 * ROWS).map(|k| peek(&m, top.wrapping_add(k))).collect();
        let sfr = |a: usize| m.cpu.sfr[a - 0x80] as u32;
        let bank = (sfr(0xD0) as usize >> 3 & 3) * 8;
        let mut registers = vec![
            ("A".to_string(), sfr(0xE0)),
            ("B".to_string(), sfr(0xF0)),
            ("PSW".to_string(), sfr(0xD0)),
            ("SP".to_string(), sfr(0x81)),
            ("DPTR".to_string(), sfr(0x83) << 8 | sfr(0x82)),
            ("PC".to_string(), m.cpu.pc as u32),
        ];
        registers.extend((0..8).map(|r| (format!("R{r}"), m.cpu.iram[bank + r] as u32)));
        let mut at = dbg.code_at.map_or(m.cpu.pc, |a| a as u16);
        let mut code = Vec::new();
        for _ in 0..24 {
            let (text, len) = ribbit_leappad::disasm::disasm(at, |a| m.bus.code_peek(a));
            code.push((at as u32, len as u32, text));
            at = at.wrapping_add(len.max(1));
        }
        Snapshot {
            system: "LeapPad".into(),
            regions,
            bytes,
            registers,
            pc: Some(m.cpu.pc as u32),
            code,
            breakpoints: breakpoints.to_vec(),
            paused,
            can_step: true,
        }
    }

    /// Writes a byte for the debugger (the RAMs only); runs one instruction;
    /// sets the breakpoints and continues past one.
    pub fn debug_poke(&self, addr: u32, v: u8) {
        let mut m = self.shared.m.lock().unwrap_or_else(|e| e.into_inner());
        match addr {
            0x2000_F000..=0x2000_FFFF => m.bus.xram[(addr - 0x2000_F000) as usize] = v,
            0x3000_0000..=0x3000_00FF => m.cpu.iram[(addr - 0x3000_0000) as usize] = v,
            0x3100_0080..=0x3100_00FF => m.cpu.sfr[(addr - 0x3100_0080) as usize] = v,
            _ => {}
        }
    }

    pub fn debug_step(&self) {
        let mut m = self.shared.m.lock().unwrap_or_else(|e| e.into_inner());
        m.step();
    }

    pub fn set_breakpoints(&self, breakpoints: &[u32], resume: bool) {
        let mut m = self.shared.m.lock().unwrap_or_else(|e| e.into_inner());
        m.breakpoints = breakpoints.iter().map(|&b| b as u16).collect();
        m.resume |= resume;
    }

    /// Starts the speech model's download (the bar's button).
    fn download_words_model(&mut self) {
        #[cfg(not(target_os = "android"))]
        if let Some(data) = &self.words_data
            && self.words_download.is_none()
        {
            self.words_download = Some(crate::leappad_words::download_model(data));
        }
    }

    pub fn close_book(&mut self) {
        self.book = None;
        self.book_problems.clear();
        self.pictures.clear();
    }

    /// Stops the machine and keeps its EEPROM.
    pub fn stop(mut self) {
        self.shared.quit.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let data = self.shared.m.lock().unwrap_or_else(|e| e.into_inner()).bus.eeprom.data.clone();
        if let Some(dir) = self.eeprom_path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&self.eeprom_path, data) {
            tracing::warn!(target: "ribbit::leappad", "saving the EEPROM: {e}");
        }
    }

    pub fn speed(&self) -> f32 {
        f32::from_bits(self.shared.speed.load(Ordering::Relaxed))
    }

    /// Stops (or restarts) the machine; its clock goes on from where it was.
    pub fn set_paused(&self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Relaxed);
    }

    pub fn set_gain(&self, gain: f32) {
        self.shared.gain.store(gain.to_bits(), Ordering::Relaxed);
    }

    fn set_pen(&mut self, pen: Option<(Spot, bool)>) {
        self.pen = pen;
        *self.shared.pen.lock().unwrap() = pen.filter(|p| p.1).map(|p| p.0);
    }

    fn picture(&mut self, ctx: &egui::Context, side: Side) -> Option<egui::TextureHandle> {
        let path = self.book.as_ref()?.picture(self.spread, side)?;
        self.pictures
            .entry(path.clone())
            .or_insert_with(|| {
                let img = load_png(&path)?;
                Some(ctx.load_texture(path.display().to_string(), img, egui::TextureOptions::LINEAR))
            })
            .clone()
    }
}

impl Drop for LeapPadSession {
    fn drop(&mut self) {
        self.shared.quit.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// A byte for the debugger at its address (`LeapPadSession::debug_snapshot`).
fn peek(m: &Machine, addr: u32) -> Option<u8> {
    match addr {
        0x2000_F000..=0x2000_FFFF => m.bus.xram.get((addr - 0x2000_F000) as usize).copied(),
        0x3000_0000..=0x3000_00FF => Some(m.cpu.iram[(addr - 0x3000_0000) as usize]),
        0x3100_0080..=0x3100_00FF => Some(m.cpu.sfr[(addr - 0x3100_0080) as usize]),
        _ => m.bus.peek_phys(addr),
    }
}

fn load_png(path: &Path) -> Option<egui::ColorImage> {
    let file = std::fs::File::open(path).ok()?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(file));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = dec.read_info().ok()?;
    let mut buf = vec![0; r.output_buffer_size()?];
    let info = r.next_frame(&mut buf).ok()?;
    let px = &buf[..info.buffer_size()];
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => px.to_vec(),
        png::ColorType::Rgb => px.chunks(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => px.chunks(2).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
        png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    Some(egui::ColorImage::from_rgba_unmultiplied([info.width as usize, info.height as usize], &rgba))
}

/// The emulation thread: runs the machine in 5 ms steps, paced to real time.
fn run(s: &Shared) {
    let step = MACHINE_HZ / 200;
    let mut origin = Instant::now();
    let mut done: u64 = 0;
    let mut partial = String::new();
    let mut meter = (Instant::now(), 0u64);
    let mut audio = Vec::new();
    // What happened in the last 5 s, for the log (sound reaching the output,
    // pen presses, touches the cartridge felt).
    let mut report = (Instant::now(), 0usize, 0i16, 0u32, 0u32, false, 0.0f32);
    // The longest wait for the machine's lock, the longest step, and the
    // longest time between two steps, in the last 5 s.
    let mut slow = (Duration::ZERO, Duration::ZERO, Duration::ZERO);
    let mut last_step: Option<Instant> = None;
    while !s.quit.load(Ordering::Relaxed) {
        if s.paused.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(10));
            origin = Instant::now();
            done = 0;
            continue;
        }
        let due = (origin.elapsed().as_secs_f64() * MACHINE_HZ as f64) as u64;
        if done >= due {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        if due - done > MACHINE_HZ as u64 / 2 {
            // Far behind (the PC was busy): carry on from now.
            origin = Instant::now();
            done = 0;
        }
        let step_start = Instant::now();
        if let Some(t) = last_step {
            slow.2 = slow.2.max(step_start - t);
        }
        last_step = Some(step_start);
        let text = {
            let mut m = s.m.lock().unwrap_or_else(|e| e.into_inner());
            slow.0 = slow.0.max(step_start.elapsed());
            let pen = *s.pen.lock().unwrap();
            if pen.is_some() && !report.5 {
                report.3 += 1;
            }
            report.5 = pen.is_some();
            m.set_pen(pen);
            if s.power.swap(false, Ordering::Relaxed) {
                m.press_power();
            }
            m.run_cycles(step);
            if let Some(h) = m.hit.take() {
                *s.hit.lock().unwrap() = Some(h);
                s.paused.store(true, Ordering::Relaxed);
            }
            s.off.store(m.is_off(), Ordering::Relaxed);
            audio.resize(m.bus.sound.output.len(), 0);
            m.bus.sound.output.pop_into(&mut audio);
            m.take_log()
        };
        slow.1 = slow.1.max(step_start.elapsed());
        report.1 += audio.len();
        report.2 = report.2.max(audio.iter().map(|v| v.saturating_abs()).max().unwrap_or(0));
        report.6 = f32::from_bits(s.gain.load(Ordering::Relaxed));
        match s.sink.lock().unwrap().as_mut() {
            Some(sink) => sink.push(&audio, report.6),
            None => report.6 = -1.0,
        }
        done += step as u64;
        if !text.is_empty() {
            partial.push_str(&text);
            let mut lines = s.log.lock().unwrap();
            while let Some(nl) = partial.find('\n') {
                let line: String = partial.drain(..=nl).collect();
                let line = line.trim_end().to_string();
                for t in book::touch_reports(&line) {
                    report.4 += 1;
                    let mut touches = s.touches.lock().unwrap();
                    touches.push_back((Instant::now(), t));
                    if touches.len() > 20 {
                        touches.pop_front();
                    }
                }
                if !line.is_empty() {
                    // The cartridge's own messages (touches, views, sounds):
                    // what to look at when a touch seems not to answer.
                    tracing::debug!(target: "ribbit::leappad::firmware", "{line}");
                    lines.push_back(line);
                }
                if lines.len() > LOG_LINES {
                    lines.pop_front();
                }
            }
        }
        let elapsed = meter.0.elapsed().as_secs_f64();
        if elapsed >= 1.0 {
            let pct = 100.0 * (done.saturating_sub(meter.1)) as f64 / (elapsed * MACHINE_HZ as f64);
            s.speed.store((pct as f32).to_bits(), Ordering::Relaxed);
            meter = (Instant::now(), done);
        }
        if report.0.elapsed().as_secs() >= 5 {
            let (_, samples, peak, presses, felt, _, gain) = report;
            if presses > 0 || peak > 64 {
                tracing::info!(
                    target: "ribbit::leappad",
                    "last 5 s: sound samples={samples} peak={peak} gain={gain} (-1: no output) pen presses={presses} touches felt={felt} \
                     longest lock wait={}ms step={}ms between steps={}ms",
                    slow.0.as_millis(),
                    slow.1.as_millis(),
                    slow.2.as_millis()
                );
            }
            slow = (Duration::ZERO, Duration::ZERO, Duration::ZERO);
            report = (Instant::now(), 0, 0, 0, 0, report.5, gain);
        }
    }
}

// The 1999 LeapPad's colours (teal-and-white box art, docs/leappad.md "Look").
const BODY: egui::Color32 = egui::Color32::from_rgb(28, 150, 150);
const BODY_DARK: egui::Color32 = egui::Color32::from_rgb(18, 104, 108);
const RIM: egui::Color32 = egui::Color32::from_rgb(236, 244, 242);
const PLATFORM: egui::Color32 = egui::Color32::from_rgb(196, 230, 226);
const PAPER: egui::Color32 = egui::Color32::from_rgb(255, 250, 236);
const INK: egui::Color32 = egui::Color32::from_rgb(70, 60, 50);
const GO_GREEN: egui::Color32 = egui::Color32::from_rgb(40, 170, 70);
/// Outlines of a book drawn from the cartridge, and the one under the pen.
const OUTLINE: egui::Color32 = egui::Color32::from_rgb(120, 140, 170);
const HOT: egui::Color32 = egui::Color32::from_rgb(230, 120, 20);
const PEN_BLUE: egui::Color32 = egui::Color32::from_rgb(40, 90, 200);

/// Draws the LeapPad in `area` and handles the pen; returns what the app
/// should do.
pub fn show(ui: &mut egui::Ui, area: egui::Rect, lp: &mut LeapPadSession, paused: bool, book_only: &mut bool, debug: &mut bool) -> Vec<Event> {
    let mut events = Vec::new();
    let p = ui.painter().clone();
    p.rect_filled(area, 0.0, egui::Color32::from_rgb(34, 36, 40));
    // Controls along the bottom, and above them (while there is news) the
    // words' row: in the bar it was pushed off a normal-sized window.
    let bar_h = 34.0;
    let bar = egui::Rect::from_min_max(egui::pos2(area.min.x, area.max.y - bar_h), area.max);
    let words_h = if lp.words_status.is_empty() { 0.0 } else { 30.0 };
    let words_row = egui::Rect::from_min_max(egui::pos2(area.min.x, bar.min.y - words_h), egui::pos2(area.max.x, bar.min.y));
    let room = egui::Rect::from_min_max(area.min, egui::pos2(area.max.x, words_row.min.y)).shrink(10.0);
    // The spread's size: two portrait pages side by side.
    let spread_aspect = 2.0 * PAGE_W / PAGE_H;
    let spread = if *book_only {
        fit(room, spread_aspect)
    } else {
        // The pad: the book platform plus the rim and the right-hand bar.
        let pad = fit(room, spread_aspect * 1.0 / 0.80 * 0.86);
        draw_pad(&p, pad);
        let w = pad.width();
        let platform = egui::Rect::from_min_max(pad.min + egui::vec2(w * 0.035, pad.height() * 0.07), egui::pos2(pad.max.x - w * 0.17, pad.max.y - pad.height() * 0.07));
        p.rect_filled(platform, 10.0, PLATFORM);
        fit(platform.shrink(6.0), spread_aspect)
    };
    let ctx = ui.ctx().clone();
    lp.poll_words(&ctx);
    for side in [Side::Left, Side::Right] {
        let page = page_rect(spread, side);
        p.rect_filled(page.translate(egui::vec2(3.0, 3.0)), 3.0, egui::Color32::from_black_alpha(60));
        p.rect_filled(page, 3.0, PAPER);
        if let Some(tex) = lp.picture(&ctx, side) {
            p.image(tex.id(), page, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
        }
    }
    // The spiral binding.
    let rings = 14;
    for i in 0..rings {
        let y = spread.min.y + spread.height() * (i as f32 + 0.5) / rings as f32;
        p.circle_stroke(egui::pos2(spread.center().x, y), spread.height() * 0.018, egui::Stroke::new(2.0, egui::Color32::from_gray(150)));
    }
    let to_screen = |s: Spot| {
        let page = page_rect(spread, s.page);
        egui::pos2(page.min.x + s.x / PAGE_W * page.width(), page.min.y + s.y / PAGE_H * page.height())
    };
    let word_rect = |r: &book::Region| {
        let [x0, y0, x1, y1] = r.rect;
        egui::Rect::from_two_pos(to_screen(Spot { page: r.page, x: x0, y: y0 }), to_screen(Spot { page: r.page, x: x1, y: y1 }))
    };
    let font = egui::FontId::proportional((spread.height() / 50.0).clamp(9.0, 16.0));
    let has_pictures = lp.book.as_ref().is_some_and(|b| {
        b.spreads.get(lp.spread).is_some_and(|s| s.left.is_some() || s.right.is_some())
    });
    match lp.book.as_ref().and_then(|b| b.spreads.get(lp.spread)) {
        Some(s) => {
            let drawn = lp.book.as_ref().is_some_and(|b| b.from_cartridge);
            if drawn {
                // The cartridge knows only the spots' shapes, not the
                // printed art: picture shapes get soft colours (largest
                // first, so details sit on top), words stay on the paper,
                // and other boxes (buttons) are tinted.
                let mut order: Vec<&book::Region> = s.regions.iter().collect();
                let area = |r: &book::Region| (r.rect[2] - r.rect[0]) * (r.rect[3] - r.rect[1]);
                order.sort_by(|a, b| area(b).total_cmp(&area(a)));
                // One text size for the page's words: the median of the sizes
                // that fit each word's box (a word too long for its box at
                // that size is shrunk on its own). Page numbers size to their
                // boxes.
                let text_size = {
                    let mut fits: Vec<f32> = s
                        .regions
                        .iter()
                        .filter(|r| printed_word(r) && !r.text.chars().all(|c| c.is_ascii_digit()))
                        .map(|r| word_fit(&p, word_rect(r), &r.text))
                        .collect();
                    fits.sort_by(f32::total_cmp);
                    fits.get(fits.len() / 2).copied()
                };
                for r in order {
                    let hot = lp.pen.is_some_and(|(at, _)| r.contains(at));
                    let word = printed_word(r);
                    let colour = shape_colour(&r.name);
                    let edge = if hot {
                        egui::Stroke::new(2.5, HOT)
                    } else if word {
                        egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(120, 140, 170, 90))
                    } else {
                        egui::Stroke::new(1.2, darker(colour))
                    };
                    if r.outline.is_empty() {
                        let rect = word_rect(r);
                        if !word {
                            p.rect_filled(rect, 4.0, colour.gamma_multiply(0.55));
                        }
                        p.rect_stroke(rect, if word { 0.0 } else { 4.0 }, edge, egui::StrokeKind::Inside);
                    } else {
                        let pts: Vec<egui::Pos2> = r.outline.iter().map(|&[x, y]| to_screen(Spot { page: r.page, x, y })).collect();
                        if !word {
                            fill_polygon(&p, &pts, colour);
                        }
                        p.add(egui::Shape::closed_line(pts, edge));
                    }
                    if word {
                        let number = r.text.chars().all(|c| c.is_ascii_digit());
                        draw_word(&p, word_rect(r), &r.text, if number { None } else { text_size });
                    }
                    if r.sentence {
                        draw_speech_bubble(&p, word_rect(r), hot);
                    }
                    if r.stop {
                        draw_stop_sign(&p, word_rect(r), hot);
                    }
                }
                // What the spot under the pen says, when it is more than its
                // printed word: the smallest spot there (a word inside a
                // picture is the word, not the picture), and none over words.
                let under = lp.pen.and_then(|(at, _)| {
                    s.regions.iter().filter(|r| r.contains(at)).min_by(|a, b| area(a).total_cmp(&area(b)))
                });
                if let Some(said) = under
                    .filter(|r| !printed_word(r) && !r.stop && !(r.said.is_empty() && r.text.is_empty()))
                    .map(|r| if r.said.is_empty() { r.text.clone() } else { r.said.clone() })
                {
                    let at = to_screen(lp.pen.unwrap().0);
                    let galley = p.layout(format!("“{said}”"), font.clone(), INK, spread.width() * 0.3);
                    let box_ = egui::Rect::from_min_size(at + egui::vec2(14.0, 14.0), galley.size()).expand(4.0);
                    p.rect_filled(box_, 4.0, egui::Color32::from_rgba_unmultiplied(255, 252, 230, 235));
                    p.rect_stroke(box_, 4.0, egui::Stroke::new(1.0, OUTLINE), egui::StrokeKind::Inside);
                    p.galley(box_.min + egui::vec2(4.0, 4.0), galley, INK);
                }
            } else if !has_pictures || *debug {
                for r in &s.regions {
                    let [x0, y0, x1, y1] = r.rect;
                    let rect = egui::Rect::from_two_pos(to_screen(Spot { page: r.page, x: x0, y: y0 }), to_screen(Spot { page: r.page, x: x1, y: y1 }));
                    let hot = lp.pen.is_some_and(|(at, _)| r.contains(at));
                    p.rect_filled(rect, 4.0, if hot { egui::Color32::from_rgb(255, 228, 150) } else { egui::Color32::from_rgb(232, 240, 250) });
                    p.rect_stroke(rect, 4.0, egui::Stroke::new(1.0, INK), egui::StrokeKind::Inside);
                    p.text(rect.center(), egui::Align2::CENTER_CENTER, &r.name, font.clone(), INK);
                }
            }
            if let Some(go) = s.go {
                let c = to_screen(go);
                let rad = spread.height() * 0.03;
                p.circle_filled(c, rad, GO_GREEN);
                p.text(c, egui::Align2::CENTER_CENTER, "GO", egui::FontId::proportional(rad * 0.9), egui::Color32::WHITE);
            }
        }
        None => {
            let page = page_rect(spread, Side::Left);
            let msg = if lp.cart.is_some() {
                let why = lp.layout_note.as_ref().map_or(String::new(), |n| format!("\n\nRibbit could not draw this cartridge's book: {n}."));
                format!(
                    "No virtual book is open.\n\nThe cartridge is running: touch the pages and it answers as \
                     it would on the real pad, but there is nothing to see here.{why}\n\nOpen a book (Emulation > LeapPad > \
                     Open book), or Ribbit's test book."
                )
            } else {
                "The pad is running its own built-in book. No virtual book is open.".to_string()
            };
            let galley = p.layout(msg, font.clone(), INK, page.width() * 0.85);
            p.galley(page.center() - galley.size() / 2.0, galley, INK);
        }
    }
    // The pen: the mouse over the book; pressed, it is down on the page.
    let resp = ui.interact(spread, ui.id().with("leappad-book"), egui::Sense::click_and_drag());
    let hover = resp.hover_pos().or(resp.interact_pointer_pos());
    let spot = hover.and_then(|h| book::spot_at((h.x - spread.min.x) / spread.width(), (h.y - spread.min.y) / spread.height()));
    let down = resp.is_pointer_button_down_on();
    match scripted_pen(lp) {
        // Paused, the pen is lifted (the pad stays drawn).
        _ if paused => lp.set_pen(None),
        Some(pen) => lp.set_pen(pen),
        None => lp.set_pen(spot.map(|s| (s, down))),
    }
    if lp.script.is_some() {
        // Between touches too, or a touch's moment can pass between frames.
        ui.ctx().request_repaint_after(Duration::from_millis(30));
    }
    if let Some((s, down)) = lp.pen {
        let at = to_screen(s);
        p.circle_stroke(at, 9.0, egui::Stroke::new(2.0, PEN_BLUE));
        if down {
            p.circle_filled(at, 5.0, PEN_BLUE);
        }
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    if !*book_only {
        draw_pen(&p, area, spread, lp.pen.map(|(s, _)| to_screen(s)));
    }
    if lp.shared.off.load(Ordering::Relaxed) {
        p.rect_filled(spread, 3.0, egui::Color32::from_black_alpha(150));
        p.text(spread.center(), egui::Align2::CENTER_CENTER, "The LeapPad is off.\nPress ON to turn it on.", egui::FontId::proportional(22.0), egui::Color32::WHITE);
    }
    // The bar.
    let mut bar_ui = ui.new_child(egui::UiBuilder::new().max_rect(bar.shrink2(egui::vec2(10.0, 4.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let n = lp.book.as_ref().map_or(0, |b| b.spreads.len());
    if bar_ui.add_enabled(lp.spread > 0, egui::Button::new("◀")).on_hover_text("Previous page (the pen touches its GO)").clicked() {
        lp.spread -= 1;
        if lp.auto_go {
            touch_go(lp);
        }
    }
    let name = lp.book.as_ref().and_then(|b| b.spreads.get(lp.spread)).map_or("(no book)".to_string(), |s| s.name.clone());
    bar_ui.label(egui::RichText::new(format!("{name}  {}/{}", (lp.spread + 1).min(n), n)).color(egui::Color32::WHITE));
    if bar_ui.add_enabled(lp.spread + 1 < n, egui::Button::new("▶")).on_hover_text("Next page (the pen touches its GO)").clicked() {
        lp.spread += 1;
        if lp.auto_go {
            touch_go(lp);
        }
    }
    bar_ui.separator();
    if bar_ui.button("Book…").clicked() {
        events.push(Event::PickBook);
    }
    if bar_ui.button("Test book").on_hover_text("Ribbit's pen test book: numbered targets, to check where the cartridge feels each touch").clicked() {
        events.push(Event::TestBook);
    }
    if lp.cart.is_some()
        && bar_ui
            .button("From cartridge")
            .on_hover_text("A book Ribbit draws from the cartridge's own layout: the outline of every spot it answers, and the GO spots")
            .clicked()
    {
        events.push(Event::CartridgeBook);
    }
    if lp.book.is_some() && bar_ui.button("Close book").clicked() {
        events.push(Event::CloseBook);
    }
    bar_ui.separator();
    if bar_ui.button(egui::RichText::new("ON/OFF").strong()).on_hover_text("The pad's ON/OFF switch").clicked() {
        lp.shared.power.store(true, Ordering::Relaxed);
    }
    bar_ui.checkbox(book_only, "Book only");
    bar_ui.checkbox(debug, "Debug");
    bar_ui.label(egui::RichText::new(format!("{:.0}%", lp.speed())).weak().color(egui::Color32::LIGHT_GRAY));
    if !lp.words_status.is_empty() {
        let mut row = ui.new_child(
            egui::UiBuilder::new().max_rect(words_row.shrink2(egui::vec2(10.0, 2.0))).layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        // Spinning while the words load or the model downloads.
        if lp.words_job.is_some() || lp.words_download.is_some() {
            row.spinner();
        }
        row.label(egui::RichText::new(&lp.words_status).weak().color(egui::Color32::LIGHT_GRAY));
        #[cfg(not(target_os = "android"))]
        if lp.words_job.is_none()
            && lp.words_download.is_none()
            && lp.words_data.as_deref().is_some_and(|d| !crate::leappad_words::model_ready(d))
            && row
                .button(format!("Get it ({} MB)", crate::leappad_words::MODEL_MB))
                .on_hover_text(
                    "Cartridges hold no text, so Ribbit finds a book's words by listening to every spot on this PC, with OpenAI's Whisper speech model (English, downloaded once from Hugging Face). The words are kept, so each book is only transcribed once.",
                )
                .clicked()
        {
            lp.download_words_model();
        }
    }
    if *debug {
        debug_window(&ctx, lp);
    }
    events
}

/// The pen as the touch script has it now (None: no script running, the
/// mouse is the pen). Turns to the touch's spread on the way.
fn scripted_pen(lp: &mut LeapPadSession) -> Option<Option<(Spot, bool)>> {
    let (started, touches) = lp.script.as_mut()?;
    let now = started.elapsed().as_secs_f32();
    while touches.front().is_some_and(|t| now > t.at + SCRIPT_HOVER + SCRIPT_DOWN) {
        touches.pop_front();
    }
    let Some(t) = touches.front() else {
        lp.script = None;
        return Some(None);
    };
    if now < t.at {
        return Some(None);
    }
    let (spread, region, down) = (t.spread, t.region, now >= t.at + SCRIPT_HOVER);
    let s = lp.book.as_ref()?.spreads.get(spread)?;
    let spot = match region {
        None => s.go?,
        Some(r) => {
            let r = s.regions.get(r)?;
            let [x0, y0, x1, y1] = r.rect;
            Spot { page: r.page, x: (x0 + x1) / 2.0, y: (y0 + y1) / 2.0 }
        }
    };
    if lp.spread != spread {
        lp.spread = spread;
    }
    Some(Some((spot, down)))
}

/// After a page turn: the pen touches the new spread's GO, as a child would,
/// so the cartridge knows the page (scripted, so the pen is seen doing it).
fn touch_go(lp: &mut LeapPadSession) {
    let (started, touches) = lp.script.get_or_insert_with(|| (Instant::now(), VecDeque::new()));
    let at = started.elapsed().as_secs_f32();
    // A quick touch: the hover is skipped, so the press is at once.
    touches.retain(|t| t.region.is_some());
    touches.push_back(ScriptTouch { at: at - SCRIPT_HOVER, spread: lp.spread, region: None });
}

/// Soft picture-book colours for the shapes of a book drawn from the
/// cartridge, picked by the spot's name so a shape keeps its colour.
fn shape_colour(name: &str) -> egui::Color32 {
    const COLOURS: [(u8, u8, u8); 10] = [
        (244, 170, 140), (250, 210, 120), (160, 210, 150), (140, 195, 230), (200, 170, 225),
        (245, 185, 200), (180, 220, 215), (230, 200, 160), (175, 190, 240), (215, 225, 140),
    ];
    let h = name.bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
    let (r, g, b) = COLOURS[(h % COLOURS.len() as u32) as usize];
    egui::Color32::from_rgba_unmultiplied(r, g, b, 170)
}

fn darker(c: egui::Color32) -> egui::Color32 {
    egui::Color32::from_rgb((c.r() as f32 * 0.55) as u8, (c.g() as f32 * 0.55) as u8, (c.b() as f32 * 0.6) as u8)
}

/// Fills a simple polygon, concave or not (ear clipping).
fn fill_polygon(p: &egui::Painter, pts: &[egui::Pos2], colour: egui::Color32) {
    let n = pts.len();
    if n < 3 {
        return;
    }
    let cross = |a: egui::Pos2, b: egui::Pos2, c: egui::Pos2| (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    // Winding: positive area means the ears turn the same way.
    let area: f32 = (0..n).map(|i| cross(egui::Pos2::ZERO, pts[i], pts[(i + 1) % n])).sum();
    let sign = if area >= 0.0 { 1.0 } else { -1.0 };
    let mut idx: Vec<usize> = (0..n).collect();
    let mut mesh = egui::Mesh::default();
    for &q in pts {
        mesh.colored_vertex(q, colour);
    }
    let mut guard = 0;
    while idx.len() > 3 && guard < 4 * n * n {
        guard += 1;
        let m = idx.len();
        let mut clipped = false;
        for k in 0..m {
            let (ia, ib, ic) = (idx[(k + m - 1) % m], idx[k], idx[(k + 1) % m]);
            let (a, b, c) = (pts[ia], pts[ib], pts[ic]);
            if cross(a, b, c) * sign <= 0.0 {
                continue;
            }
            let inside = idx.iter().any(|&j| {
                j != ia && j != ib && j != ic && {
                    let q = pts[j];
                    cross(a, b, q) * sign > 0.0 && cross(b, c, q) * sign > 0.0 && cross(c, a, q) * sign > 0.0
                }
            });
            if inside {
                continue;
            }
            mesh.add_triangle(ia as u32, ib as u32, ic as u32);
            idx.remove(k);
            clipped = true;
            break;
        }
        if !clipped {
            break;
        }
    }
    if idx.len() == 3 {
        mesh.add_triangle(idx[0] as u32, idx[1] as u32, idx[2] as u32);
    }
    p.add(egui::Shape::mesh(mesh));
}

/// A spot's word, as large as fits in its box.
/// Whether a spot is drawn as a printed word: a box of a line's height with a
/// transcribed word, or a page number's box (taller: 109-110 units in the
/// Pooh book and At the Shore). A larger spot that says a word (a page's
/// background saying "Thanks") is a picture.
fn printed_word(r: &book::Region) -> bool {
    let height = r.rect[3] - r.rect[1];
    let number = r.text.chars().all(|c| c.is_ascii_digit());
    !r.text.is_empty() && r.outline.is_empty() && (height < book::PAGE_H * 0.085 || (number && height < book::PAGE_H * 0.12))
}

/// The page's stop sign: a red octagon with STOP on it.
fn draw_stop_sign(p: &egui::Painter, rect: egui::Rect, hot: bool) {
    let r = rect.width().min(rect.height()) * 0.42;
    let c = rect.center();
    let corner = |k: usize, rad: f32| {
        let a = std::f32::consts::PI / 8.0 + k as f32 * std::f32::consts::PI / 4.0;
        c + egui::vec2(a.cos(), a.sin()) * rad
    };
    let red = if hot { egui::Color32::from_rgb(235, 70, 60) } else { egui::Color32::from_rgb(200, 30, 35) };
    p.add(egui::Shape::convex_polygon((0..8).map(|k| corner(k, r)).collect(), egui::Color32::WHITE, egui::Stroke::NONE));
    p.add(egui::Shape::convex_polygon((0..8).map(|k| corner(k, r * 0.88)).collect(), red, egui::Stroke::NONE));
    p.text(c, egui::Align2::CENTER_CENTER, "STOP", egui::FontId::proportional(r * 0.55), egui::Color32::WHITE);
}

/// A sentence's button: a speech bubble (a round box with a tail and lines
/// of "text"), as the books print it before the sentence.
fn draw_speech_bubble(p: &egui::Painter, rect: egui::Rect, hot: bool) {
    let size = rect.width().min(rect.height()) * 0.8;
    let body = egui::Rect::from_center_size(rect.center() - egui::vec2(0.0, size * 0.08), egui::vec2(size, size * 0.66));
    let fill = if hot { egui::Color32::from_rgb(255, 236, 170) } else { egui::Color32::WHITE };
    let edge = egui::Stroke::new((size * 0.06).max(1.0), egui::Color32::from_rgb(40, 110, 190));
    let tail = vec![
        body.left_bottom() + egui::vec2(size * 0.22, -1.0),
        body.left_bottom() + egui::vec2(size * 0.12, size * 0.26),
        body.left_bottom() + egui::vec2(size * 0.42, -1.0),
    ];
    p.add(egui::Shape::convex_polygon(tail.clone(), fill, edge));
    p.rect_filled(body, size * 0.22, fill);
    p.rect_stroke(body, size * 0.22, edge, egui::StrokeKind::Inside);
    // Cover the tail's top edge where it joins the box.
    p.line_segment([tail[0] + egui::vec2(edge.width, -edge.width), tail[2] + egui::vec2(-edge.width, -edge.width)], egui::Stroke::new(edge.width * 1.2, fill));
    for k in 0..3 {
        let y = body.top() + body.height() * (0.3 + 0.2 * k as f32);
        let w = body.width() * if k == 2 { 0.4 } else { 0.62 };
        p.line_segment([egui::pos2(body.left() + body.width() * 0.19, y), egui::pos2(body.left() + body.width() * 0.19 + w, y)], egui::Stroke::new((size * 0.05).max(1.0), egui::Color32::from_rgb(120, 150, 190)));
    }
}

/// The largest text size at which `text` fits its box.
fn word_fit(p: &egui::Painter, rect: egui::Rect, text: &str) -> f32 {
    let width = p.layout_no_wrap(text.to_string(), egui::FontId::proportional(10.0), INK).size().x.max(1.0);
    (rect.height() * 0.7).min(10.0 * rect.width() * 0.92 / width).clamp(6.0, 40.0)
}

/// A printed word in its box, at `size` (the page's) unless it would not fit
/// (then at the size that fits); `None`: the size that fits.
fn draw_word(p: &egui::Painter, rect: egui::Rect, text: &str, size: Option<f32>) {
    let fit = word_fit(p, rect, text);
    let size = size.map_or(fit, |s| s.min(fit));
    let galley = p.layout_no_wrap(text.to_string(), egui::FontId::proportional(size), INK);
    p.galley(rect.center() - galley.size() / 2.0, galley, INK);
}

fn fit(room: egui::Rect, aspect: f32) -> egui::Rect {
    let (w, h) = if room.width() / room.height() > aspect { (room.height() * aspect, room.height()) } else { (room.width(), room.width() / aspect) };
    egui::Rect::from_center_size(room.center(), egui::vec2(w, h))
}

fn page_rect(spread: egui::Rect, side: Side) -> egui::Rect {
    let half = spread.width() / 2.0;
    let x0 = spread.min.x + if side == Side::Left { 0.0 } else { half };
    egui::Rect::from_min_size(egui::pos2(x0, spread.min.y), egui::vec2(half, spread.height()))
}

/// The pad's body: a landscape lap desk, teal with a white rim, the right
/// side a raised bar with the speaker grille and the name, spiral posts at
/// the top and bottom of the spine, an oval pen rest at the bottom right.
fn draw_pad(p: &egui::Painter, pad: egui::Rect) {
    let r = pad.height() * 0.06;
    p.rect_filled(pad.translate(egui::vec2(0.0, 6.0)), r, egui::Color32::from_black_alpha(90));
    p.rect_filled(pad, r, RIM);
    p.rect_filled(pad.shrink(pad.height() * 0.025), r * 0.8, BODY);
    let w = pad.width();
    let bar = egui::Rect::from_min_max(egui::pos2(pad.max.x - w * 0.15, pad.min.y + pad.height() * 0.05), egui::pos2(pad.max.x - w * 0.02, pad.max.y - pad.height() * 0.05));
    p.rect_filled(bar, r * 0.6, BODY_DARK);
    // Speaker grille: slots near the top of the bar.
    for i in 0..7 {
        let y = bar.min.y + bar.height() * (0.08 + i as f32 * 0.035);
        p.line_segment([egui::pos2(bar.min.x + bar.width() * 0.2, y), egui::pos2(bar.max.x - bar.width() * 0.2, y)], egui::Stroke::new(bar.height() * 0.012, egui::Color32::from_rgb(10, 60, 64)));
    }
    p.text(egui::pos2(bar.center().x, bar.min.y + bar.height() * 0.45), egui::Align2::CENTER_CENTER, "Ribbit", egui::FontId::proportional(bar.width() * 0.2), egui::Color32::from_rgb(250, 220, 60));
    // The pen rest.
    let rest = egui::Rect::from_center_size(egui::pos2(bar.center().x, bar.max.y - bar.height() * 0.15), egui::vec2(bar.width() * 0.7, bar.height() * 0.12));
    p.rect_filled(rest, rest.height() / 2.0, egui::Color32::from_rgb(12, 80, 84));
    // Spiral posts at the top and bottom of the spine.
    let spine = pad.min.x + (pad.width() * 0.83 + pad.width() * 0.035) / 2.0;
    for y in [pad.min.y + pad.height() * 0.04, pad.max.y - pad.height() * 0.04] {
        p.rect_filled(egui::Rect::from_center_size(egui::pos2(spine, y), egui::vec2(pad.width() * 0.05, pad.height() * 0.05)), 6.0, BODY_DARK);
    }
}

/// The tethered pen: its cord from the bottom post, and the pen at the
/// cursor (or in its rest).
fn draw_pen(p: &egui::Painter, area: egui::Rect, spread: egui::Rect, at: Option<egui::Pos2>) {
    let post = egui::pos2(spread.center().x, spread.max.y + spread.height() * 0.06);
    let tip = at.unwrap_or_else(|| egui::pos2(area.max.x - area.width() * 0.08, spread.max.y - spread.height() * 0.06));
    let pts: Vec<egui::Pos2> = (0..=24)
        .map(|i| {
            let t = i as f32 / 24.0;
            let sag = (t * std::f32::consts::PI).sin() * 30.0;
            post.lerp(tip, t) + egui::vec2(0.0, sag)
        })
        .collect();
    p.add(egui::Shape::line(pts, egui::Stroke::new(3.0, PEN_BLUE.gamma_multiply(0.8))));
    if at.is_none() {
        p.line_segment([tip, tip + egui::vec2(-40.0, -14.0)], egui::Stroke::new(8.0, PEN_BLUE));
    }
}

fn debug_window(ctx: &egui::Context, lp: &LeapPadSession) {
    egui::Window::new("LeapPad debug").default_width(420.0).default_pos(egui::pos2(40.0, 60.0)).show(ctx, |ui| {
        let (cycles, counter, unknown, calibrated) = {
            let m = lp.shared.m.lock().unwrap_or_else(|e| e.into_inner());
            (m.cpu.cycles, m.bus.counter, m.bus.unknown.len(), ribbit_leappad::calibration::is_calibrated(&m.bus.eeprom.data))
        };
        ui.label(format!("Emulated time {:.1} s  •  clock counter {counter}  •  {:.0}% speed", cycles as f64 / MACHINE_HZ as f64, lp.speed()));
        ui.label(format!("EEPROM calibration: {}  •  accesses not understood: {unknown}", if calibrated { "valid" } else { "missing" }));
        match lp.pen {
            Some((s, down)) => {
                let (b, a) = book::pen_ratios(s);
                ui.label(format!("Pen {} on the {:?} page at ({:.0}, {:.0})  •  sensor ratios B {:.3}, A {:.3}", if down { "down" } else { "over" }, s.page, s.x, s.y, b, a));
            }
            None => {
                ui.label("Pen away from the book");
            }
        }
        ui.separator();
        ui.label(egui::RichText::new("Touches the cartridge reported (its debug log)").strong());
        let touches = lp.shared.touches.lock().unwrap();
        for (t, r) in touches.iter().rev().take(6) {
            let region = lp
                .book
                .as_ref()
                .and_then(|b| b.spreads.get(lp.spread))
                .and_then(|s| s.regions.iter().find(|g| g.contains(r.spot)))
                .map_or(String::new(), |g| format!("  — in \"{}\"", g.name));
            ui.label(format!("{:>5.1} s ago: {:?} page ({:.0}, {:.0}){region}", t.elapsed().as_secs_f32(), r.spot.page, r.spot.x, r.spot.y));
        }
        drop(touches);
        if !lp.book_problems.is_empty() {
            ui.separator();
            ui.label(egui::RichText::new("Book file").strong());
            for p in &lp.book_problems {
                ui.label(p.to_string());
            }
        }
        ui.separator();
        ui.label(egui::RichText::new("Firmware log").strong());
        egui::ScrollArea::vertical().max_height(260.0).stick_to_bottom(true).show(ui, |ui| {
            let log = lp.shared.log.lock().unwrap();
            for l in log.iter() {
                ui.label(egui::RichText::new(l).monospace().size(11.0));
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_part_numbers() {
        assert_eq!(part_number(Path::new("(LeapPad, 500-00406) Tutter (USA).bin")).as_deref(), Some("500-00406"));
        assert_eq!(part_number(Path::new("game.bin")), None);
    }

    #[test]
    fn the_test_book_parses() {
        let (b, _) = Book::parse(TEST_BOOK, Path::new(".")).expect("valid");
        assert_eq!(b.spreads.len(), 3);
    }
}

#[cfg(test)]
mod title_tests {
    #[test]
    fn book_titles_drop_the_archive_prefix() {
        assert_eq!(
            super::book_title("(LeapPad, 500-00652) Leap 1 Reading - Disney's Bounce, Tigger, Bounce (USA)"),
            "Leap 1 Reading - Disney's Bounce, Tigger, Bounce (USA)"
        );
        assert_eq!(super::book_title("(Quantum LeapPad, 500-00975-B) The Wind (USA)"), "The Wind (USA)");
        assert_eq!(super::book_title("At the Shore"), "At the Shore");
    }

    #[test]
    fn series_come_before_the_first_dash() {
        assert_eq!(
            super::book_series("Leap Start Pre Reading - Tutter's Tiny Trip (USA)"),
            (Some("Leap Start Pre Reading".into()), "Tutter's Tiny Trip (USA)".into())
        );
        assert_eq!(
            super::book_series("Leap 1 Reading - Disney's Bounce, Tigger, Bounce (USA)"),
            (Some("Leap 1 Reading".into()), "Disney's Bounce, Tigger, Bounce (USA)".into())
        );
        assert_eq!(super::book_series("Leap's Pond Magazine Premier Issue (USA)"), (None, "Leap's Pond Magazine Premier Issue (USA)".into()));
    }
}
