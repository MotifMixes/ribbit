//! The game library: cartridges found in the player's ROM folders, with
//! each game's own title card as its art.
//!
//! A background thread scans the folders, then makes any missing art by
//! booting the game unseen (no sound, no saves written) and keeping its most
//! colourful screen of the first seconds, which is the game's title card.
//! Art is cached as PNG in the data folder, so each game is booted once.
//! With a BIOS whose Flash rasterizer Ribbit recognises, art is captured in
//! HD (640x640, ribbit_core::hd); otherwise at the LCD's 160x160.

use ribbit_core::Machine;
use ribbit_debug::Breakpoints;
use ribbit_memory::Cartridge;
use ribbit_video::{HEIGHT, WIDTH};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

const EXTENSIONS: [&str; 5] = ["bin", "rom", "u3", "lst", "zip"];
/// Folder depth scanned below each ROM folder.
const MAX_DEPTH: usize = 3;
/// Art: emulated time searched for the title card, and how often to look.
const ART_SEARCH_MS: u64 = 16_000;
const ART_SAMPLE_MS: u64 = 250;
/// Games' art made at once (each boots a machine on its own thread): all
/// cores but two, so the first library scan is quick.
fn art_threads() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get().saturating_sub(2).max(1))
}
/// How often a capture hands its finished renders to its HD picture.
const HD_DRAIN_MS: u64 = 50;

/// When each known game shows its title card with no input (emulated ms
/// from power-on, US BIOS v2.1, the built-in ART_PROFILE), by CRC32. Found
/// by recording each game. Games not listed are searched for their most
/// colourful early screen.
const TITLE_CARD_MS: &[(u32, u64)] = &[
    (0x0ef6_1cbb, 13_500), // Spider-Man - The Case of the Sinister Speller
    (0xd4a7_e5d4, 7_000), // Disney/Pixar Cars (stops after its LeapFrog logo, which is its art)
    (0x0f2a_d725, 9_000), // SpongeBob SquarePants - Saves the Day (USA, alt)
    (0x0f6d_55cf, 10_500), // Number Raiders
    (0x19da_3dd8, 11_250), // Scooby-Doo! - Spooky Snacks
    (0x2465_f01d, 24_000), // Disney Princess - Worlds of Enchantment
    (0x2700_0674, 12_750), // Star Wars - The Clone Wars - Jedi Math
    (0x29af_42ba, 24_000), // Cars 2
    (0x2c1f_cf40, 9_750), // Learning with Leap
    (0x3c42_8a13, 22_500), // Pet Pals
    (0x5303_d366, 12_750), // Thomas & Friends - Calling All Engines!
    (0x58d0_8f36, 15_000), // The Talking Words Factory
    (0x5ccb_2ad3, 15_750), // Disney Princess - Enchanted Learning
    (0x6239_a178, 18_750), // Star Wars - Jedi Reading
    (0x6b24_a3ef, 9_000), // SpongeBob SquarePants - Through the Wormhole
    (0x7177_8536, 12_000), // Dora the Explorer - Camping Adventure
    (0x74af_5d57, 10_500), // Dora the Explorer - Wildlife Rescue
    (0x9793_46d5, 12_000), // Cosmic Math
    (0x9b86_a0fb, 12_000), // Letterpillar
    (0x9e69_c26a, 24_000), // Outwit!
    (0x9f08_d02a, 37_500), // Creature Create (no title card: the intro's monster, after taps)
    (0xa0a3_62aa, 18_000), // The Batman - Multiply, Divide and Conquer (Germany)
    (0xa9c7_5d90, 21_000), // Madagascar
    (0xa9d2_cdbd, 16_500), // Sonic X
    (0xb186_a3bf, 11_250), // Letters on the Loose
    (0xd0c4_de47, 15_750), // Ratatouille
    (0xd2f5_60c7, 12_750), // Disney Fairies (Spain)
    (0xda56_181c, 15_750), // I Spy Challenger!
    (0xdb26_2164, 9_750), // Kindergarten
    (0xdb36_283f, 9_750), // 1st Grade
    (0xe62d_1684, 12_750), // Foster's Home for Imaginary Friends
    (0xea54_4030, 11_250), // Dora the Explorer - Pinata Party
];

/// Stylus taps (emulated ms, LCD x, y) before a title card, by CRC32.
/// Creature Create asks for a name first ("TOM", OK) and has no title card:
/// its art is the intro's monster after that.
const TITLE_CARD_TAPS: &[(u32, &[(u64, u32, u32)])] =
    &[(0x9f08_d02a, &[(13_000, 74, 92), (13_700, 87, 74), (14_400, 61, 74), (15_100, 145, 141)])];
/// How long each tap holds the stylus down.
const TAP_MS: u64 = 200;

/// The system EEPROM every art capture boots with (calibration done, a
/// region chosen), so a game's title card comes at the same moment for
/// everyone: the player's own saves change the start-up (a remembered
/// region skips the region picker). Made by Ribbit; no personal data.
const ART_PROFILE: &[u8; 256] = include_bytes!("art_profile.eeprom");

/// Where a game's art is cached (the suffix changes whenever the way art
/// is made changes, so old pictures are made again).
const ART_SUFFIX: &str = "-6.png";

fn art_file(art_dir: &Path, key: &str) -> PathBuf {
    art_dir.join(format!("{key}{ART_SUFFIX}"))
}

/// Deletes art made the old way.
fn remove_stale_art(art_dir: &Path) {
    let Ok(rd) = std::fs::read_dir(art_dir) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.ends_with(".png") && !name.ends_with(ART_SUFFIX) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

pub struct Game {
    pub path: PathBuf,
    pub title: String,
    /// One line under the title: part number, year, size.
    pub subtitle: String,
    /// Label, value pairs for the details tooltip.
    pub details: Vec<(&'static str, String)>,
    key: String,
    pub art: Art,
}

pub enum Art {
    /// Not made yet (or being made).
    Pending,
    /// Can't be made (no BIOS, or the game showed nothing).
    Missing,
    Image(egui::ColorImage),
    Texture(egui::TextureHandle),
}

/// What the art maker boots the games with (and ART_PROFILE).
#[derive(Clone)]
pub struct ArtSource {
    pub bios: PathBuf,
}

enum Msg {
    Game(Game),
    Art(String, Option<egui::ColorImage>),
    Scanned,
    Done,
}

pub struct Library {
    pub games: Vec<Game>,
    pub scanning: bool,
    pub making_art: bool,
    art_dir: PathBuf,
    rx: Option<Receiver<Msg>>,
    /// A rescan's games, collected while the previous list stays on show
    /// (so the library never goes blank); swapped in once listed.
    incoming: Option<Vec<Game>>,
}

impl Library {
    pub fn new(data_dir: &Path) -> Self {
        Self { games: Vec::new(), scanning: false, making_art: false, art_dir: data_dir.join("art"), rx: None, incoming: None }
    }

    /// Rescans `folders` (plus `extra` files, e.g. recent games) in the
    /// background. Without `art`, games without cached art show none.
    pub fn scan(&mut self, folders: &[PathBuf], extra: &[PathBuf], art: Option<ArtSource>) {
        let (tx, rx) = channel();
        // The first scan fills the list as it goes; later ones replace it whole.
        self.incoming = (!self.games.is_empty()).then(Vec::new);
        self.scanning = true;
        self.making_art = false;
        self.rx = Some(rx); // a running older scan sees its receiver gone and stops
        let (folders, extra, art_dir) = (folders.to_vec(), extra.to_vec(), self.art_dir.clone());
        let spawned = std::thread::Builder::new()
            .name("ribbit-library".into())
            .spawn(move || worker(&folders, &extra, &art_dir, art.as_ref(), &tx));
        if spawned.is_err() {
            self.scanning = false;
        }
    }

    /// Takes in what the background thread found; call every frame.
    pub fn poll(&mut self) {
        let Some(rx) = &self.rx else {
            return;
        };
        while let Ok(msg) = rx.try_recv() {
            match msg {
                Msg::Game(g) => {
                    let list = self.incoming.as_mut().unwrap_or(&mut self.games);
                    let at = list.partition_point(|o| o.title.to_lowercase() <= g.title.to_lowercase());
                    list.insert(at, g);
                }
                Msg::Art(key, image) => {
                    let lists = self.games.iter_mut().chain(self.incoming.iter_mut().flatten());
                    for g in lists.filter(|g| g.key == key) {
                        g.art = image.clone().map_or(Art::Missing, Art::Image);
                    }
                }
                Msg::Scanned => {
                    if let Some(list) = self.incoming.take() {
                        self.games = list;
                    }
                    self.scanning = false;
                    self.making_art = self.games.iter().any(|g| matches!(g.art, Art::Pending));
                }
                Msg::Done => {
                    if let Some(list) = self.incoming.take() {
                        self.games = list;
                    }
                    self.scanning = false;
                    self.making_art = false;
                    for g in &mut self.games {
                        if matches!(g.art, Art::Pending) {
                            g.art = Art::Missing;
                        }
                    }
                    self.rx = None;
                    return;
                }
            }
        }
    }

    /// Forgets a game's cached art, so the next scan makes it again.
    pub fn forget_art(&mut self, path: &Path) {
        for g in self.games.iter_mut().filter(|g| g.path == path) {
            let _ = std::fs::remove_file(art_file(&self.art_dir, &g.key));
            g.art = Art::Pending;
        }
    }
}

fn worker(folders: &[PathBuf], extra: &[PathBuf], art_dir: &Path, art: Option<&ArtSource>, tx: &Sender<Msg>) {
    remove_stale_art(art_dir);
    let mut files = Vec::new();
    for f in folders {
        collect(f, 0, &mut files);
    }
    files.extend(extra.iter().filter(|p| p.is_file()).cloned());
    let mut seen_paths = HashSet::new();
    let mut seen_games = HashSet::new();
    let mut todo = Vec::new();
    for path in files {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        if !seen_paths.insert(canonical) {
            continue;
        }
        let Some(cart) = read_cartridge(&path) else {
            continue;
        };
        let key = format!("{:08x}", cart.id().crc32);
        if !seen_games.insert(key.clone()) {
            continue;
        }
        let cached = load_png(&art_file(art_dir, &key));
        let have_art = cached.is_some();
        let game = describe(&path, &cart, key.clone(), cached.map_or(Art::Pending, Art::Image));
        if tx.send(Msg::Game(game)).is_err() {
            return;
        }
        if !have_art {
            todo.push((key, cart));
        }
    }
    let _ = tx.send(Msg::Scanned);
    if let Some(src) = art {
        let todo = std::sync::Mutex::new(todo);
        std::thread::scope(|scope| {
            for _ in 0..art_threads() {
                scope.spawn(|| {
                    // (The lock is released before the work: held in a
                    // `while let`, it would make the captures take turns.)
                    loop {
                        let next = todo.lock().unwrap().pop();
                        let Some((key, cart)) = next else { break };
                        let image = make_art(src, cart);
                        if let Some(img) = &image {
                            let _ = std::fs::create_dir_all(art_dir);
                            if let Err(e) = save_png(&art_file(art_dir, &key), img) {
                                tracing::warn!(target: "ribbit::library", "saving art: {e:#}");
                            }
                        }
                        if tx.send(Msg::Art(key, image.as_ref().map(to_image))).is_err() {
                            todo.lock().unwrap().clear(); // the library was closed or rescanned
                        }
                    }
                });
            }
        });
    }
    let _ = tx.send(Msg::Done);
}

fn collect(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            if depth < MAX_DEPTH {
                collect(&p, depth + 1, out);
            }
        } else if p
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
        {
            out.push(p);
        }
    }
}

/// A Leapster cartridge image (plain or zipped), or None for anything else
/// (a BIOS, other systems' ROMs, unreadable files).
fn read_cartridge(path: &Path) -> Option<Cartridge> {
    // Zipped images are smaller on disk; the size is checked again below.
    let size = std::fs::metadata(path).ok()?.len();
    if !(0x100..=ribbit_memory::map::CART_WINDOW as u64).contains(&size) {
        return None;
    }
    let data = ribbit_core::read_image(path).ok()?;
    if data.len() < 0x1_0000 {
        return None;
    }
    // Any known BIOS (also Leapster 2 ones, which Ribbit does not load).
    if ribbit_memory::known::lookup(ribbit_memory::known::FIRMWARE, &ribbit_memory::ImageId::of(&data)).is_some() {
        return None;
    }
    // Later cartridges have no "LEAP" header; any power-of-two image of a
    // cartridge's size is taken (the BIOS decides whether it boots).
    let plausible = data.len().is_power_of_two() && data.len() >= 0x10_0000;
    let cart = Cartridge::from_bytes(data).ok()?;
    (cart.known().is_some() || cart.header().is_ok() || plausible).then_some(cart)
}

fn describe(path: &Path, cart: &Cartridge, key: String, art: Art) -> Game {
    let known = cart.known();
    let header_title = cart.header_text(0x0b);
    let part = cart.header_text(0x0a);
    let built = cart.header_text(0x0f);
    let stem = path.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    // Known dumps by name; otherwise the file name, which players usually
    // keep tidy (the header's title can be generic, e.g. "Included Title").
    let title = known
        .map(|k| crate::app::short_name(k.name))
        .or_else(|| (!stem.is_empty()).then(|| stem.clone()))
        .or(header_title.clone())
        .unwrap_or_default();
    // "Oct 09 2003 18:23:53" -> "2003".
    let year = built.as_deref().and_then(|b| b.split_whitespace().nth(2)).map(str::to_string);
    let size = format!("{} MB", cart.rom().len().div_ceil(1 << 20));
    let subtitle = [part.clone(), year, Some(size.clone())].into_iter().flatten().collect::<Vec<_>>().join("  ·  ");
    let mut details = Vec::new();
    if let Some(t) = &header_title {
        details.push(("Cartridge title", t.clone()));
    }
    if let Some(p) = part {
        details.push(("Part number", p));
    }
    if let Some(b) = built {
        details.push(("Built", b));
    }
    if let Some(k) = known {
        if let Some(cat) = k.name.split_once(" [").and_then(|(_, r)| r.split_once(']')).map(|(c, _)| c) {
            details.push(("Catalogue", cat.to_string()));
        }
        if let Some(set) = k.name.split_once("(MAME: ").and_then(|(_, r)| r.split_once(')')).map(|(s, _)| s) {
            details.push(("MAME set", set.to_string()));
        }
        details.push(("Dump", "verified (known checksum)".into()));
    } else {
        details.push(("Dump", "not in Ribbit's list (may still work)".into()));
    }
    details.push(("Size", size));
    details.push(("CRC32", key.clone()));
    details.push(("File", path.display().to_string()));
    Game { path: path.to_path_buf(), title, subtitle, details, key, art }
}

/// Boots the game unseen and returns its most colourful early screen.
/// A captured picture, `size` x `size`, 0xAARRGGBB.
struct Shot {
    size: usize,
    pixels: Vec<u32>,
}

/// A machine booting a game for its art, with its HD picture kept up to
/// date when the BIOS's rasterizer is recognised.
struct Capture {
    m: Machine,
    canvas: Option<ribbit_core::hd::HdCanvas>,
    frame: Box<[u32; WIDTH * HEIGHT]>,
}

impl Capture {
    /// Runs to `ms` from power-on; false if the machine stopped.
    fn run_to(&mut self, ms: u64) -> bool {
        let none = Breakpoints::default();
        let per_ms = self.m.clock.cpu_hz / 1000;
        let target = per_ms * ms;
        while self.m.cpu.cycles < target {
            let chunk = (self.m.cpu.cycles + per_ms * HD_DRAIN_MS).min(target);
            while self.m.cpu.cycles < chunk {
                if self.m.run(chunk - self.m.cpu.cycles, &none).stop != ribbit_core::StopReason::StepLimit {
                    return false;
                }
            }
            self.drain();
        }
        true
    }

    /// The LCD frame now, with the finished renders drawn into the HD picture.
    fn drain(&mut self) {
        self.m.render_frame(&mut self.frame);
        if let (Some(c), Some(hd)) = (self.canvas.as_mut(), self.m.hd.as_mut()) {
            for p in hd.take() {
                c.apply(&p, &self.frame[..]);
            }
        }
    }

    /// The picture now: HD when available (kept true to the LCD).
    fn shot(&mut self) -> Shot {
        self.drain();
        match self.canvas.as_mut() {
            Some(c) => {
                c.reconcile(&self.frame[..]);
                Shot { size: ribbit_core::hd::HD_SIZE, pixels: c.pixels.clone() }
            }
            None => Shot { size: WIDTH, pixels: self.frame.to_vec() },
        }
    }
}

fn make_art(src: &ArtSource, cart: Cartridge) -> Option<Shot> {
    let title_card = TITLE_CARD_MS.iter().find(|(crc, _)| *crc == cart.id().crc32).map(|&(_, ms)| ms);
    let fw = ribbit_core::load_firmware(&src.bios).ok()?;
    // Straps bit 14 set: the BIOS takes the saved calibration (no targets).
    let mut m = Machine::with_straps(fw, Some(cart), 0x63FF_FFFF);
    m.record_trace_tail = false;
    m.io.eeprom.load(ART_PROFILE);
    m.enable_experimental_mame_touch(Default::default());
    let canvas = m.enable_hd_capture().then(|| ribbit_core::hd::HdCanvas::new(m.hd.as_ref().unwrap().hooks));
    let mut cap = Capture { m, canvas, frame: Box::new([0u32; WIDTH * HEIGHT]) };
    if let Some(ms) = title_card {
        // A known game: straight to its title card.
        let crc = cap.m.cartridge().map(|c| c.id().crc32);
        let taps = TITLE_CARD_TAPS.iter().find(|(c, _)| Some(*c) == crc).map_or(&[][..], |(_, t)| t);
        // MAME's crosshair mapping (0..=0x7F0 across 160 pixels), as for the mouse.
        let raw = |p: u32| ((p * 0x7F0 + 79) / 159).min(0x7F0);
        for &(at, x, y) in taps {
            for (when, live) in [(at, Some((raw(x), raw(y)))), (at + TAP_MS, None)] {
                if !cap.run_to(when) {
                    return None;
                }
                if let Some(t) = cap.m.io.touch.as_mut() {
                    t.live = live;
                }
            }
        }
        if !cap.run_to(ms) {
            return None;
        }
        return Some(cap.shot());
    }
    // The game's own screens (once its code runs) win over the BIOS's, then
    // still frames (unchanged since the last sample) over frames caught
    // mid-animation, then the most colourful.
    let mut best: Option<((bool, bool), usize, Shot)> = None;
    let mut last = cap.frame.clone();
    for i in 1..=ART_SEARCH_MS / ART_SAMPLE_MS {
        if !cap.run_to(i * ART_SAMPLE_MS) {
            return best.map(|b| b.2);
        }
        cap.drain();
        let still = (cap.m.first_cart_pc.is_some(), *cap.frame == *last);
        last.copy_from_slice(&cap.frame[..]);
        let colours = colourfulness(&cap.frame);
        if best.as_ref().is_none_or(|b| (still, colours) > (b.0, b.1)) {
            best = Some((still, colours, cap.shot()));
        }
    }
    let best = best.map(|(_, colours, shot)| (colours, shot));
    // A blank or near-blank screen is no art.
    best.filter(|b| b.0 >= 16).map(|b| b.1)
}

/// Distinct colours on screen: high for title art, low for logos and menus.
fn colourfulness(frame: &[u32; WIDTH * HEIGHT]) -> usize {
    frame.iter().map(|p| p & 0x00FF_FFFF).collect::<HashSet<_>>().len()
}

fn to_image(shot: &Shot) -> egui::ColorImage {
    let rgb: Vec<u8> = shot.pixels.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]).collect();
    egui::ColorImage::from_rgb([shot.size, shot.size], &rgb)
}

fn save_png(path: &Path, shot: &Shot) -> anyhow::Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, shot.size as u32, shot.size as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let rgb: Vec<u8> = shot.pixels.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]).collect();
    enc.write_header()?.write_image_data(&rgb)?;
    Ok(())
}

fn load_png(path: &Path) -> Option<egui::ColorImage> {
    let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).ok()?));
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let size = [info.width as usize, info.height as usize];
    let data = &buf[..info.buffer_size()];
    match info.color_type {
        png::ColorType::Rgb => Some(egui::ColorImage::from_rgb(size, data)),
        png::ColorType::Rgba => Some(egui::ColorImage::from_rgba_unmultiplied(size, data)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_cards_beat_flat_screens() {
        let flat = Box::new([0x00FF_FFFFu32; WIDTH * HEIGHT]);
        let mut busy = Box::new([0u32; WIDTH * HEIGHT]);
        for (i, p) in busy.iter_mut().enumerate() {
            *p = (i as u32 * 7919) & 0x00FF_FFFF;
        }
        assert!(colourfulness(&busy) > colourfulness(&flat));
        assert_eq!(colourfulness(&flat), 1);
    }

    #[test]
    fn art_survives_a_png_round_trip() {
        let dir = std::env::temp_dir().join(format!("ribbit-art-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut shot = Shot { size: 640, pixels: vec![0; 640 * 640] };
        shot.pixels[0] = 0x0012_3456;
        let path = dir.join("a.png");
        save_png(&path, &shot).unwrap();
        let img = load_png(&path).unwrap();
        assert_eq!(img.size, [640, 640]);
        assert_eq!(img.pixels[0], egui::Color32::from_rgb(0x12, 0x34, 0x56));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scanning_skips_non_cartridges() {
        let dir = std::env::temp_dir().join(format!("ribbit-scan-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("notes.txt"), b"hello").unwrap();
        std::fs::write(dir.join("tiny.bin"), [0u8; 16]).unwrap();
        let mut cart = vec![0u8; 0x1_0000];
        cart[..4].copy_from_slice(b"LEAP");
        std::fs::write(dir.join("sub").join("game.bin"), &cart).unwrap();
        let mut files = Vec::new();
        collect(&dir, 0, &mut files);
        let carts: Vec<_> = files.iter().filter(|p| read_cartridge(p).is_some()).collect();
        assert_eq!(carts, [&dir.join("sub").join("game.bin")]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
