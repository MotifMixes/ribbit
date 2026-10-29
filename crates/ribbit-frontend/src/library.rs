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
use ribbit_memory::known::Hardware;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

const EXTENSIONS: [&str; 7] = ["bin", "rom", "u3", "lst", "zip", "lfp", "lf3"];
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
/// by recording each game; re-timed for the CPU timing model (2026-10-15)
/// with `examples/retitle.rs`, which matches each frame against the
/// previous art. Games not listed are searched for their most colourful
/// early screen.
/// (Moments re-timed 2026-10-16 from half-second timelines, the middle of
/// each title card: several had drifted onto a logo, a copyright screen or
/// the black frame before the title.)
const TITLE_CARD_MS: &[(u32, u64)] = &[
    (0x0ef6_1cbb, 10_250), // Spider-Man - The Case of the Sinister Speller
    (0xd4a7_e5d4, 21_000), // Disney/Pixar Cars (its "Disney presents a Pixar film" title)
    (0x0f2a_d725, 8_750), // SpongeBob SquarePants - Saves the Day (USA, alt)
    (0x0f6d_55cf, 11_750), // Number Raiders
    (0x19da_3dd8, 10_750), // Scooby-Doo! - Spooky Snacks
    (0x2465_f01d, 33_500), // Disney Princess - Worlds of Enchantment
    (0x2700_0674, 13_500), // Star Wars - The Clone Wars - Jedi Math
    (0x29af_42ba, 26_000), // Cars 2
    (0x2c1f_cf40, 10_500), // Learning with Leap
    (0x3c42_8a13, 33_500), // Pet Pals
    (0x5303_d366, 15_000), // Thomas & Friends - Calling All Engines!
    (0x58d0_8f36, 15_750), // The Talking Words Factory
    (0x5ccb_2ad3, 15_750), // Disney Princess - Enchanted Learning
    (0x6239_a178, 24_000), // Star Wars - Jedi Reading
    // Through the Wormhole has no title screen of its own (as recorded on a
    // Leapster TV: LeapFrog, the Nickelodeon/SpongeBob legal screen, then
    // the player menu); its player menu, before the buttons start pulsing.
    (0x6b24_a3ef, 11_000), // SpongeBob SquarePants - Through the Wormhole
    (0x7177_8536, 12_250), // Dora the Explorer - Camping Adventure
    (0x74af_5d57, 12_500), // Dora the Explorer - Wildlife Rescue
    (0x9793_46d5, 12_250), // Cosmic Math
    // NASCAR has no title card: its NASCAR logo screen, after the race intro.
    (0x95c7_8f34, 23_000), // NASCAR
    (0x9b86_a0fb, 12_000), // Letterpillar
    (0x9e69_c26a, 33_000), // Outwit!
    (0x9f08_d02a, 9_000), // Creature Create
    (0xa0a3_62aa, 19_000), // The Batman - Multiply, Divide and Conquer (Germany)
    (0xa9c7_5d90, 22_000), // Madagascar
    (0xa9d2_cdbd, 15_500), // Sonic X
    (0xb186_a3bf, 11_750), // Letters on the Loose
    (0xd0c4_de47, 16_000), // Ratatouille
    (0xd2f5_60c7, 12_500), // Disney Fairies (Spain)
    (0xda56_181c, 18_500), // I Spy Challenger!
    (0xdb26_2164, 8_500), // Kindergarten
    (0xdb36_283f, 11_500), // 1st Grade
    (0xe62d_1684, 11_250), // Foster's Home for Imaginary Friends
    (0xea54_4030, 10_750), // Dora the Explorer - Pinata Party
];

/// Stylus taps (emulated ms, LCD x, y) before a title card, by CRC32 (none
/// needed since the CPU timing model: Creature Create's title card now shows
/// before its name entry).
const TITLE_CARD_TAPS: &[(u32, &[(u64, u32, u32)])] = &[];
/// How long each tap holds the stylus down.
const TAP_MS: u64 = 200;

/// The system EEPROM every art capture boots with (calibration done, a
/// region chosen), so a game's title card comes at the same moment for
/// everyone: the player's own saves change the start-up (a remembered
/// region skips the region picker). Made by Ribbit; no personal data.
const ART_PROFILE: &[u8; 256] = include_bytes!("art_profile.eeprom");

/// Where a game's art is cached (the suffix changes whenever the way art
/// is made changes, so old pictures are made again).
const ART_SUFFIX: &str = "-10.png";

fn art_file(art_dir: &Path, key: &str) -> PathBuf {
    art_dir.join(art_name(key))
}

/// A game's art file name: its CRC32 (`key`), and for games with a known
/// title card its moment, so re-timing a game makes its art again.
fn art_name(key: &str) -> String {
    match title_card_ms(key) {
        Some(ms) => format!("{key}-t{ms}{ART_SUFFIX}"),
        None => format!("{key}{ART_SUFFIX}"),
    }
}

fn title_card_ms(key: &str) -> Option<u64> {
    let crc = u32::from_str_radix(key, 16).ok()?;
    TITLE_CARD_MS.iter().find(|(c, _)| *c == crc).map(|&(_, ms)| ms)
}

/// Deletes art made the old way.
fn remove_stale_art(art_dir: &Path) {
    let Ok(rd) = std::fs::read_dir(art_dir) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        // Old suffixes, and listed games' art from an earlier title moment.
        let key = name.split(['-', '.']).next().unwrap_or_default();
        let current = name == art_name(key) || title_card_ms(key).is_none();
        if name.ends_with(".png") && (!name.ends_with(ART_SUFFIX) || !current) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Which consoles a game runs on (and the library's tabs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// A Leapster cartridge: the Leapster, Leapster TV, L-MAX and Leapster 2.
    Leapster,
    /// A cartridge made for the Leapster 2 only (LEAPSTER2_ONLY).
    Leapster2,
    /// A Zippity cartridge (a FAT16 image with KERNEL.BIN).
    Zippity,
    /// A RockIt Twist package (.lfp system package, encrypted .lf3 pack).
    RockItTwist,
    /// A cartridge of the original LeapPad (1999 book-and-pen pad).
    LeapPad,
    /// A Didj package: the console's firmware (its menu) or a game.
    Didj,
}

/// Cartridges made for the Leapster 2 alone, by CRC32: the "Leapster 2
/// Gaming Pop" kiosk cartridge (MAME: lgamingpop; its file is named "Leapster
/// 2 Cartridge 2010"). Whether the original Leapster boots it is UNVERIFIED.
const LEAPSTER2_ONLY: &[u32] = &[0xc6fd_5c24];

impl Family {
    /// The library tabs: the game consoles in alphabetical order, then the
    /// LeapPad (a book reader; Ribbit is a game emulator first).
    pub const ALL: [Family; 6] = [Family::Didj, Family::Leapster, Family::Leapster2, Family::RockItTwist, Family::Zippity, Family::LeapPad];

    /// A book reader's (the library's Books side), not a game console's.
    pub fn is_book(self) -> bool {
        self == Family::LeapPad
    }

    pub fn name(self) -> &'static str {
        match self {
            Family::Leapster => "Leapster",
            Family::Leapster2 => "Leapster 2",
            Family::Zippity => "Zippity",
            Family::RockItTwist => "RockIt Twist",
            Family::LeapPad => "LeapPad",
            Family::Didj => "Didj",
        }
    }

    /// Whether a console of this hardware runs the family's games.
    pub fn runs_on(self, hw: Hardware) -> bool {
        match self {
            Family::Leapster => !matches!(hw, Hardware::Zippity | Hardware::RockItTwist | Hardware::LeapPad | Hardware::Didj),
            Family::Leapster2 => hw == Hardware::Leapster2,
            Family::Zippity => hw == Hardware::Zippity,
            Family::RockItTwist => hw == Hardware::RockItTwist,
            Family::LeapPad => hw == Hardware::LeapPad,
            Family::Didj => hw == Hardware::Didj,
        }
    }

    /// The console a tab is about.
    pub fn console(self) -> Hardware {
        match self {
            Family::Leapster => Hardware::Leapster,
            Family::Leapster2 => Hardware::Leapster2,
            Family::Zippity => Hardware::Zippity,
            Family::RockItTwist => Hardware::RockItTwist,
            Family::LeapPad => Hardware::LeapPad,
            Family::Didj => Hardware::Didj,
        }
    }

    /// Whether a tab's page lists a game of family `game`.
    pub fn lists(self, game: Family) -> bool {
        match self {
            // The Leapster 2's page: its own games only (the Leapster's are
            // on theirs).
            Family::Leapster2 => game == Family::Leapster2,
            _ => game.runs_on(self.console()),
        }
    }
}

/// The consoles, in the order the library lists them.
pub const CONSOLES: [Hardware; 6] = [
    Hardware::Leapster,
    Hardware::LeapsterTv,
    Hardware::LeapsterLmax,
    Hardware::Leapster2,
    Hardware::Zippity,
    Hardware::RockItTwist,
];

pub struct Game {
    pub path: PathBuf,
    pub family: Family,
    pub title: String,
    /// One line under the title: part number, year, size.
    pub subtitle: String,
    /// Label, value pairs for the details tooltip.
    pub details: Vec<(&'static str, String)>,
    /// Badges shown with the game ("Internal": built into the console;
    /// "Prototype" and its build date).
    pub tags: Vec<std::borrow::Cow<'static, str>>,
    pub(crate) key: String,
    pub art: Art,
}

impl Game {
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }
}

/// A prototype or pre-release build, going by its name: these dumps are not
/// in the known list, and players name them after what they are ("Wall-E
/// December 20th 2007 prototype", "ROX Alpha 1.15", "1.8 Beta",
/// "Animation Sample 1").
fn is_prototype(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|w| matches!(w, "prototype" | "proto" | "beta" | "alpha" | "sample" | "submission" | "unreleased" | "j2s"))
}

/// The build date compiled into the program ("Dec 20 2007 16:28:29", C's
/// `__DATE__ __TIME__`) as "Dec 20, 2007"; the latest one when there are
/// several (libraries carry their own). Prototypes show it as a tag, since
/// the date is what tells one build from another.
fn build_date(rom: &[u8]) -> Option<String> {
    const MONTHS: [&[u8]; 12] = [b"Jan", b"Feb", b"Mar", b"Apr", b"May", b"Jun", b"Jul", b"Aug", b"Sep", b"Oct", b"Nov", b"Dec"];
    let digits = |b: &[u8]| b.iter().all(u8::is_ascii_digit);
    let mut best: Option<(u32, usize, u32)> = None;
    // "Mmm dd yyyy hh:mm:ss": 20 bytes; the day is space-padded.
    for w in rom.windows(20) {
        let Some(month) = MONTHS.iter().position(|m| w[..3] == **m) else { continue };
        let day = &w[4..6];
        let ok = w[3] == b' '
            && (day[0] == b' ' || day[0].is_ascii_digit())
            && day[1].is_ascii_digit()
            && w[6] == b' '
            && digits(&w[7..11])
            && w[11] == b' '
            && digits(&w[12..14])
            && w[14] == b':'
            && digits(&w[15..17])
            && w[17] == b':'
            && digits(&w[18..20]);
        if !ok {
            continue;
        }
        let year: u32 = std::str::from_utf8(&w[7..11]).ok()?.parse().ok()?;
        let day: u32 = std::str::from_utf8(day).ok()?.trim().parse().ok()?;
        if !(1990..2100).contains(&year) || !(1..=31).contains(&day) {
            continue;
        }
        let key = (year, month, day);
        if best.is_none_or(|b| key > b) {
            best = Some(key);
        }
    }
    best.map(|(y, m, d)| format!("{} {d}, {y}", std::str::from_utf8(MONTHS[m]).unwrap()))
}

const MONTH_NAMES: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// "2008-04-23" (a file name's) or "Apr 23 2008 10:00:00" (a header's) as
/// "Apr 23, 2008".
fn pretty_date(s: &str) -> Option<String> {
    let s = s.trim();
    let num = |t: &str| t.parse::<u32>().ok();
    if let [y, m, d] = s.split('-').collect::<Vec<_>>()[..] {
        let (y, m, d) = (num(y)?, num(m)?, num(d)?);
        return (s.len() == 10 && y >= 1990 && (1..=12).contains(&m) && (1..=31).contains(&d))
            .then(|| format!("{} {d}, {y}", MONTH_NAMES[m as usize - 1]));
    }
    let mut w = s.split_whitespace();
    let (m, d, y) = (w.next()?, num(w.next()?)?, num(w.next()?)?);
    (MONTH_NAMES.contains(&m) && (1..=31).contains(&d) && y >= 1990).then(|| format!("{m} {d}, {y}"))
}

/// A header's version field ("v1.0", "A04, v1.0", "v 1.1", "1") as "v1.0";
/// None for anything else ("Example do not ship or release").
fn header_version(s: &str) -> Option<String> {
    let s = s.trim().to_ascii_lowercase();
    let number = |t: &str| {
        let n: String = t.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        let n = n.trim_end_matches('.').to_string();
        n.starts_with(|c: char| c.is_ascii_digit()).then_some(n)
    };
    let tidy = |n: String| if n.contains('.') { format!("v{n}") } else { format!("v{n}.0") };
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) {
        return Some(tidy(s));
    }
    s.match_indices('v').find_map(|(i, _)| {
        let word_start = i == 0 || !s.as_bytes()[i - 1].is_ascii_alphabetic();
        word_start.then(|| number(s[i + 1..].trim_start())).flatten().map(tidy)
    })
}

/// What a tidy file name says after the title, read from the right (a game
/// collection's names, 2026-10-25): "Cars - Supercharged (USA) (Leapster2)
/// (Proto) (Alpha 002) (2008-04-23)" gives the region, editions, "Proto",
/// the version and the build date, each optional. Read from the right
/// because a title can hold parentheses of its own; reading stops at the
/// region, or at a group it does not know, which stays in the title.
#[derive(Debug, Default, PartialEq)]
struct NameTags {
    title: String,
    region: Option<String>,
    extras: Vec<String>,
    prototype: bool,
    version: Option<String>,
    date: Option<String>,
}

const REGIONS: &[&str] = &[
    "USA", "UK", "Europe", "France", "Germany", "Spain", "Canada", "Australia", "Netherlands", "Italy", "Mexico", "World",
];
const EDITIONS: &[&str] = &["L-MAX", "Leapster2", "Leapster 2", "POP LP7"];

fn name_tags(name: &str) -> NameTags {
    let mut t = NameTags { title: name.trim().to_string(), ..Default::default() };
    while let Some((head, group)) = t.title.strip_suffix(')').and_then(|r| r.rsplit_once(" (")) {
        let (head, group) = (head.to_string(), group.trim().to_string());
        let lower = group.to_ascii_lowercase();
        let is_version = ["v", "alpha ", "beta ", "pre-alpha "].iter().any(|p| lower.starts_with(p))
            && group.chars().any(|c| c.is_ascii_digit());
        if group.contains('-') && pretty_date(&group).is_some() && t.date.is_none() {
            t.date = Some(group);
        } else if is_version && t.version.is_none() {
            t.version = Some(group);
        } else if lower == "proto" || lower == "prototype" {
            t.prototype = true;
        } else if EDITIONS.iter().any(|e| e.eq_ignore_ascii_case(&group)) {
            t.extras.insert(0, group);
        } else if REGIONS.iter().any(|r| r.eq_ignore_ascii_case(&group)) {
            t.region = Some(group);
            t.title = head;
            break;
        } else {
            break;
        }
        t.title = head;
    }
    t
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
    Bios(BiosFile),
    Art(String, Option<egui::ColorImage>),
    Scanned,
    Done,
}

/// A BIOS (system image) Ribbit can use.
#[derive(Debug, Clone, PartialEq)]
pub struct BiosFile {
    pub hardware: Hardware,
    pub path: PathBuf,
    /// Which dump it is ("US v2.1 (152-11265)"; the file name when unknown).
    pub name: String,
}

impl BiosFile {
    /// The name without its part numbers ("US v2.1"), for buttons.
    pub fn short_name(&self) -> &str {
        self.name.split(" (").next().unwrap_or(&self.name)
    }
}

/// A known dump's name without the console and "BaseROM" in front:
/// "Leapster BaseROM US v2.1 (152-11265)" -> "US v2.1 (152-11265)".
fn bios_name(full: &str, hw: Hardware) -> String {
    let rest = full.strip_prefix(hw.name()).unwrap_or(full).trim_start();
    rest.strip_prefix("BaseROM").unwrap_or(rest).trim_start().to_string()
}

pub struct Library {
    pub games: Vec<Game>,
    /// BIOS (system) images found in the games folders, every one of them.
    pub bios: Vec<BiosFile>,
    /// Every BIOS opened with File > Open BIOS, the one in use first (an
    /// unknown dump counts as an original Leapster's).
    opened: Vec<BiosFile>,
    /// The BIOS picked for each console (console name -> file), from the
    /// settings.
    picked: std::collections::BTreeMap<String, PathBuf>,
    pub scanning: bool,
    pub making_art: bool,
    art_dir: PathBuf,
    rx: Option<Receiver<Msg>>,
    /// A rescan's games, collected while the previous list stays on show
    /// (so the library never goes blank); swapped in once listed.
    incoming: Option<Vec<Game>>,
    /// Ribbit's own folder for the RockIt Twist's system packages.
    twist_system_dir: PathBuf,
    /// The RockIt Twist's system packages are there (in the games folders
    /// or Ribbit's own folder): the console can start.
    pub twist_ready: bool,
}

impl Library {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            games: Vec::new(),
            bios: Vec::new(),
            opened: Vec::new(),
            picked: Default::default(),
            scanning: false,
            making_art: false,
            art_dir: data_dir.join("art"),
            rx: None,
            incoming: None,
            twist_system_dir: crate::twist::system_dir(data_dir),
            twist_ready: false,
        }
    }

    /// Rescans `folders` (plus `extra` files, e.g. recent games) in the
    /// background. Without `art`, games without cached art show none.
    /// `twist_content`: the RockIt Twist content folder, whose installed
    /// games are listed too.
    pub fn scan(&mut self, folders: &[PathBuf], extra: &[PathBuf], twist_content: Option<PathBuf>, art: Option<ArtSource>) {
        let (tx, rx) = channel();
        // The first scan fills the list as it goes; later ones replace it whole.
        self.incoming = (!self.games.is_empty()).then(Vec::new);
        self.scanning = true;
        self.making_art = false;
        self.rx = Some(rx); // a running older scan sees its receiver gone and stops
        let (folders, extra, art_dir) = (folders.to_vec(), extra.to_vec(), self.art_dir.clone());
        let spawned = std::thread::Builder::new()
            .name("ribbit-library".into())
            .spawn(move || worker(&folders, &extra, twist_content.as_deref(), &art_dir, art.as_ref(), &tx));
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
                Msg::Bios(b) => {
                    if !self.bios.iter().any(|o| o.path == b.path) {
                        self.bios.push(b);
                    }
                }
                Msg::Art(key, image) => {
                    let lists = self.games.iter_mut().chain(self.incoming.iter_mut().flatten());
                    for g in lists.filter(|g| g.key == key) {
                        g.art = image.clone().map_or(Art::Missing, Art::Image);
                    }
                }
                Msg::Scanned => {
                    let mut known = crate::twist::packages_in(&self.twist_system_dir);
                    let games = self.incoming.as_ref().unwrap_or(&self.games);
                    known.extend(games.iter().filter(|g| g.family == Family::RockItTwist).map(|g| g.path.clone()));
                    self.twist_ready = crate::twist::SystemPackages::find(None, &known).is_some();
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

    /// Notes every BIOS opened with File > Open BIOS (the one in use first),
    /// so their consoles can be played on like one whose BIOS is in a games
    /// folder.
    pub fn set_opened_bios(&mut self, opened: &[PathBuf]) {
        self.opened = opened.iter().map(|p| {
            // A Zippity's system NAND: the Zippity's BIOS.
            if let Some(Found::ZippityNand(game)) = classify(p) {
                return BiosFile { hardware: Hardware::Zippity, path: p.to_path_buf(), name: game.title };
            }
            let known = ribbit_core::load_firmware(p).ok().and_then(|f| f.known().copied());
            let hardware = known.map_or(Hardware::Leapster, |k| k.hardware);
            let name = known.map_or_else(
                || p.file_stem().map_or_else(|| "Unknown dump".to_string(), |n| n.to_string_lossy().into_owned()),
                |k| bios_name(k.name, k.hardware),
            );
            BiosFile { hardware, path: p.to_path_buf(), name }
        }).collect();
    }

    /// Notes the BIOS picked for each console (console name -> file).
    pub fn set_picked_bios(&mut self, picked: &std::collections::BTreeMap<String, PathBuf>) {
        self.picked = picked.clone();
    }

    /// Every BIOS available for `hw`: those opened with File > Open BIOS
    /// (the one in use first), then those in the games folders.
    pub fn bios_list(&self, hw: Hardware) -> Vec<&BiosFile> {
        let mut list: Vec<&BiosFile> = Vec::new();
        for b in self.opened.iter().chain(&self.bios).filter(|b| b.hardware == hw) {
            if !list.iter().any(|o| o.path == b.path) {
                list.push(b);
            }
        }
        list
    }

    /// The BIOS `hw` plays on: the one picked for it (while it is still
    /// there), else the one chosen with File > Open BIOS when it is that
    /// console's, else the first found in the games folders.
    pub fn bios_file(&self, hw: Hardware) -> Option<&BiosFile> {
        let list = self.bios_list(hw);
        let picked = self.picked.get(hw.name()).and_then(|p| list.iter().find(|b| &b.path == p));
        picked.or(list.first()).copied()
    }

    /// The path of `bios_file(hw)`.
    pub fn bios_for(&self, hw: Hardware) -> Option<&Path> {
        self.bios_file(hw).map(|b| b.path.as_path())
    }

    /// Whether any console's system files are here (a BIOS or Zippity NAND
    /// in the games folders, the RockIt Twist's system): the library shows
    /// then even without a Leapster BIOS.
    pub fn has_system_files(&self) -> bool {
        self.twist_ready || CONSOLES.into_iter().any(|hw| self.bios_for(hw).is_some())
    }

    /// Whether this console can start: its BIOS (or the RockIt Twist's
    /// system packages) is here. The Leapster's page always shows: it is
    /// where a BIOS is asked for.
    pub fn has_console(&self, hw: Hardware) -> bool {
        match hw {
            Hardware::Leapster => true,
            Hardware::RockItTwist => self.twist_ready,
            // Its base ROM is kept in the settings (File > Open BIOS); a
            // book played without one says so.
            Hardware::LeapPad => true,
            // Started from its packages (it says which are missing).
            Hardware::Didj => true,
            _ => self.bios_for(hw).is_some(),
        }
    }

    /// The consoles Leapster games can play on: the Leapster models with a
    /// BIOS (chosen with File > Open BIOS or in a games folder); the
    /// original Leapster when there is none at all. Zippity and RockIt Twist
    /// games always play on their own console, so those are not choices.
    pub fn consoles(&self) -> Vec<Hardware> {
        let have: Vec<Hardware> = CONSOLES
            .into_iter()
            .filter(|&hw| !matches!(hw, Hardware::Zippity | Hardware::RockItTwist | Hardware::LeapPad | Hardware::Didj))
            .filter(|&hw| self.bios_for(hw).is_some())
            .collect();
        if have.is_empty() { vec![Hardware::Leapster] } else { have }
    }

    /// The console chosen in the library (`chosen`, a console name), or the
    /// first available one when that one is not.
    pub fn selected_console(&self, chosen: &str) -> Hardware {
        let consoles = self.consoles();
        consoles.iter().copied().find(|hw| hw.name() == chosen).unwrap_or(consoles[0])
    }

    /// The tab that lists a console's games (the Leapster 2 plays the
    /// Leapster's: its own page has only the games for it alone).
    pub fn tab_for(&self, hw: Hardware) -> Family {
        match hw {
            Hardware::Zippity => Family::Zippity,
            Hardware::RockItTwist => Family::RockItTwist,
            Hardware::LeapPad => Family::LeapPad,
            Hardware::Didj => Family::Didj,
            _ => Family::Leapster,
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

fn worker(
    folders: &[PathBuf],
    extra: &[PathBuf],
    twist_content: Option<&Path>,
    art_dir: &Path,
    art: Option<&ArtSource>,
    tx: &Sender<Msg>,
) {
    remove_stale_art(art_dir);
    for game in twist_content.map(crate::twist::installed_games).unwrap_or_default() {
        if tx.send(Msg::Game(game)).is_err() {
            return;
        }
    }
    let mut files = Vec::new();
    for f in folders {
        collect(f, 0, &mut files);
    }
    files.extend(extra.iter().filter(|p| p.is_file()).cloned());
    let mut seen_paths = HashSet::new();
    let mut seen_games = HashSet::new();
    let mut todo = Vec::new();
    let mut zippity_todo = Vec::new();
    // Zippity cartridges whose art is made after the scan (it needs a
    // Zippity system NAND, which may be found later), and the NANDs found.
    let mut zippity_cart_todo: Vec<(String, PathBuf)> = Vec::new();
    let mut zippity_nands: Vec<PathBuf> = Vec::new();
    // BIOSes whose built-in games need a picture (key, file).
    let mut builtin_todo: Vec<(String, PathBuf)> = Vec::new();
    // Original Leapster BIOSes found (art is made on one: see below).
    let mut leapster_bios: Vec<PathBuf> = vec![];
    for path in files {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        if !seen_paths.insert(canonical) {
            continue;
        }
        let cart = match classify(&path) {
            Some(Found::Cart(c)) => c,
            Some(Found::Bios(hw, name)) => {
                if hw == Hardware::Leapster {
                    leapster_bios.push(path.clone());
                }
                let bios = BiosFile { hardware: hw, path: path.clone(), name: bios_name(name, hw) };
                // Its built-in games get a card of their own.
                if bios_has_games(hw, name) && seen_games.insert(format!("bios:{}", bios.name)) {
                    let mut game = describe_builtin(&path, &bios);
                    match load_png(&art_file(art_dir, &game.key)) {
                        Some(img) => game.art = Art::Image(img),
                        None => {
                            game.art = Art::Pending;
                            builtin_todo.push((game.key.clone(), path.clone()));
                        }
                    }
                    if tx.send(Msg::Game(game)).is_err() {
                        return;
                    }
                }
                if tx.send(Msg::Bios(bios)).is_err() {
                    return;
                }
                continue;
            }
            Some(Found::ZippityNand(mut game)) => {
                zippity_nands.push(path.clone());
                let bios = BiosFile { hardware: Hardware::Zippity, path: path.clone(), name: game.title.clone() };
                if tx.send(Msg::Bios(bios)).is_err() {
                    return;
                }
                if !seen_games.insert(game.key.clone()) {
                    continue;
                }
                match load_png(&art_file(art_dir, &game.key)) {
                    Some(img) => game.art = Art::Image(img),
                    None => {
                        game.art = Art::Pending;
                        zippity_todo.push((game.key.clone(), path.clone()));
                    }
                }
                if tx.send(Msg::Game(game)).is_err() {
                    return;
                }
                continue;
            }
            Some(Found::Zippity(mut game)) => {
                if !seen_games.insert(game.key.clone()) {
                    continue;
                }
                match load_png(&art_file(art_dir, &game.key)) {
                    Some(img) => game.art = Art::Image(img),
                    None => {
                        game.art = Art::Pending;
                        zippity_cart_todo.push((game.key.clone(), path.clone()));
                    }
                }
                if tx.send(Msg::Game(game)).is_err() {
                    return;
                }
                continue;
            }
            Some(Found::Twist(game) | Found::LeapPad(game) | Found::Didj(game)) => {
                if seen_games.insert(game.key.clone()) && tx.send(Msg::Game(game)).is_err() {
                    return;
                }
                continue;
            }
            None => continue,
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
    // The Zippity's built-in games: a picture from the console itself.
    for (key, path) in zippity_todo {
        let image = make_zippity_art(&path);
        if let Some(img) = &image {
            let _ = std::fs::create_dir_all(art_dir);
            if let Err(e) = save_png(&art_file(art_dir, &key), img) {
                tracing::warn!(target: "ribbit::library", "saving art: {e:#}");
            }
        }
        if tx.send(Msg::Art(key, image.as_ref().map(to_image))).is_err() {
            return;
        }
    }
    // Zippity cartridges: their title screen, on a Zippity NAND found in the
    // folders (none: no art).
    for (key, path) in zippity_cart_todo {
        let image = zippity_nands.first().and_then(|nand| make_zippity_cart_art(nand, &path));
        if let Some(img) = &image {
            let _ = std::fs::create_dir_all(art_dir);
            if let Err(e) = save_png(&art_file(art_dir, &key), img) {
                tracing::warn!(target: "ribbit::library", "saving art: {e:#}");
            }
        }
        if tx.send(Msg::Art(key, image.as_ref().map(to_image))).is_err() {
            return;
        }
    }
    // BIOSes' built-in games: a picture from the BIOS itself, no cartridge.
    for (key, path) in builtin_todo {
        let image = make_bios_art(&path);
        if let Some(img) = &image {
            let _ = std::fs::create_dir_all(art_dir);
            if let Err(e) = save_png(&art_file(art_dir, &key), img) {
                tracing::warn!(target: "ribbit::library", "saving art: {e:#}");
            }
        }
        if tx.send(Msg::Art(key, image.as_ref().map(to_image))).is_err() {
            return;
        }
    }
    // Art is made on an original Leapster BIOS whose Flash player HD knows
    // (title cards are timed on one, and the art profile is its EEPROM): the
    // BIOS chosen in File > Open BIOS if it is one, else one found in the
    // folders. A Leapster 2 BIOS chosen there stops every game at its region
    // and name screens.
    let fits = |p: &Path| {
        ribbit_core::load_firmware(p).ok().is_some_and(|f| {
            f.known().is_none_or(|k| k.hardware == Hardware::Leapster)
                && ribbit_core::hd::Hooks::find(f.data(), ribbit_memory::map::BIOS_MIRROR).is_some_and(|h| h.glyph.is_some())
        })
    };
    let art_src = art.and_then(|src| {
        if fits(&src.bios) {
            Some(src.clone())
        } else {
            leapster_bios.iter().find(|p| fits(p)).map(|bios| ArtSource { bios: bios.clone() })
        }
    });
    if let Some(src) = art_src.as_ref() {
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

/// What a file in the games folders is.
enum Found {
    Cart(Cartridge),
    /// A BIOS or system image of this console, and its name.
    Bios(Hardware, &'static str),
    Zippity(Game),
    /// The Zippity's system NAND: the console, and its built-in games.
    ZippityNand(Game),
    /// A RockIt Twist package (listed; it cannot run).
    Twist(Game),
    /// A LeapPad book cartridge.
    LeapPad(Game),
    /// A Didj package: its firmware (the console) or a game.
    Didj(Game),
}

/// Zippity images: the system NAND (with spare area) and cartridges.
const ZIPPITY_NAND_SIZE: usize = 0x1080_0000;
const ZIPPITY_CART_SIZE: usize = 0x420_0000;

fn classify(path: &Path) -> Option<Found> {
    // Didj packages are named DIDJ-0x<product>-<version>.lfp; only some
    // are listed (the rest are found again beside them when one starts).
    if crate::didj::is_package(path) {
        return crate::didj::describe_package(path).map(Found::Didj);
    }
    // RockIt Twist packages go by name and first bytes (they are big, and
    // a .lfp is a zip that must not be unpacked as a ROM).
    if let Some(game) = crate::twist::describe_package(path) {
        return Some(Found::Twist(game));
    }
    // Zipped images are smaller on disk; sizes are checked again on the data.
    let size = std::fs::metadata(path).ok()?.len();
    if !(0x100..=ZIPPITY_NAND_SIZE as u64).contains(&size) {
        return None;
    }
    let data = ribbit_core::read_image(path).ok()?;
    if data.len() < 0x1_0000 {
        return None;
    }
    if ribbit_didj::cart::Cart::looks_like(&data) {
        return crate::didj::describe_cart(path, &data).map(Found::Didj);
    }
    use ribbit_memory::known;
    if data.len() == ZIPPITY_NAND_SIZE && data.starts_with(b"PGandnand") {
        let id = ribbit_memory::ImageId::of(&data);
        return Some(Found::ZippityNand(describe_zippity_nand(path, known::lookup(known::ZIPPITY_SYSTEM, &id), &id)));
    }
    if data.len() == ZIPPITY_CART_SIZE && &data[0x36..0x3B] == b"FAT16" {
        let id = ribbit_memory::ImageId::of(&data);
        return Some(Found::Zippity(describe_zippity(path, known::lookup(known::ZIPPITY_CARTRIDGES, &id), &id)));
    }
    // LeapPad images start with the same 8051 boot stub; a base ROM is not
    // a game (it is opened with File > Open BIOS).
    match ribbit_leappad::identify(&data) {
        Some(ribbit_leappad::Image::Cartridge) => return Some(Found::LeapPad(describe_leappad(path, &data))),
        Some(ribbit_leappad::Image::BaseRom) => return None,
        None => {}
    }
    if data.len() > ribbit_memory::map::CART_WINDOW as usize {
        return None;
    }
    if let Some(k) = known::lookup(known::FIRMWARE, &ribbit_memory::ImageId::of(&data)) {
        return Some(Found::Bios(k.hardware, k.name));
    }
    read_cartridge(data).map(Found::Cart)
}

/// Which consoles a game file runs on (None: not a game).
pub fn family_of(path: &Path) -> Option<Family> {
    match classify(path)? {
        Found::Cart(c) if c.known().is_some_and(|k| LEAPSTER2_ONLY.contains(&k.crc32)) => Some(Family::Leapster2),
        Found::Cart(_) => Some(Family::Leapster),
        Found::Zippity(_) | Found::ZippityNand(_) => Some(Family::Zippity),
        Found::Twist(_) => Some(Family::RockItTwist),
        Found::LeapPad(_) => Some(Family::LeapPad),
        Found::Didj(_) => Some(Family::Didj),
        Found::Bios(..) => None,
    }
}

/// A Zippity NAND dump (raw) in these folders, found by size and header
/// alone: for when one is needed before the library's scan has finished.
pub fn find_zippity_nand(folders: &[PathBuf]) -> Option<PathBuf> {
    let mut files = Vec::new();
    for f in folders {
        collect(f, 0, &mut files);
    }
    files.into_iter().find(|p| {
        use std::io::Read;
        let mut head = [0u8; 9];
        std::fs::metadata(p).is_ok_and(|m| m.len() == ZIPPITY_NAND_SIZE as u64)
            && std::fs::File::open(p).and_then(|mut f| f.read_exact(&mut head)).is_ok()
            && &head == b"PGandnand"
    })
}

/// Whether a file is a console's built-in games (the Zippity's NAND).
pub fn is_internal(path: &Path) -> bool {
    matches!(classify(path), Some(Found::ZippityNand(_)))
}

/// A Leapster cartridge image, or None for anything else.
fn read_cartridge(data: Vec<u8>) -> Option<Cartridge> {
    // Later cartridges have no "LEAP" header; any power-of-two image of a
    // cartridge's size is taken (the BIOS decides whether it boots).
    let plausible = data.len().is_power_of_two() && data.len() >= 0x10_0000;
    let cart = Cartridge::from_bytes(data).ok()?;
    (cart.known().is_some() || cart.header().is_ok() || plausible).then_some(cart)
}

/// A LeapPad cartridge, named from its file: "(LeapPad, 500-11878-B) Level 4 -
/// At the Shore (USA)" gives the title "Level 4 - At the Shore" and the part
/// number.
fn describe_leappad(path: &Path, data: &[u8]) -> Game {
    let stem = path.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let (part, rest) = match stem.strip_prefix('(').and_then(|r| r.split_once(") ")) {
        Some((inside, rest)) => (inside.split_once(", ").map(|(_, p)| p.to_string()), rest.to_string()),
        None => (None, stem.clone()),
    };
    let (title, region) = match rest.rsplit_once(" (") {
        Some((t, r)) if r.ends_with(')') => (t.to_string(), Some(r.trim_end_matches(')').to_string())),
        _ => (rest, None),
    };
    let id = ribbit_memory::ImageId::of(data);
    let size = format!("{} MB", id.size.div_ceil(1 << 20));
    let subtitle = [part.clone(), region, Some(size.clone())].into_iter().flatten().collect::<Vec<_>>().join("  ·  ");
    let key = format!("{:08x}", id.crc32);
    let mut details = vec![("System", "LeapPad".to_string())];
    if let Some(p) = part {
        details.push(("Part number", p));
    }
    details.push(("Size", size));
    details.push(("CRC32", key.clone()));
    details.push(("File", path.display().to_string()));
    Game { path: path.to_path_buf(), family: Family::LeapPad, title, subtitle, details, tags: Vec::new(), key, art: Art::Missing }
}

fn describe_zippity(path: &Path, known: Option<&ribbit_memory::known::KnownImage>, id: &ribbit_memory::ImageId) -> Game {
    let stem = path.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    // "500-13590-A - Name" -> "Name".
    let from_file = stem.split_once(" - ").filter(|(p, _)| p.starts_with("500-")).map_or(stem.as_str(), |(_, n)| n);
    let title = known.map_or_else(|| from_file.to_string(), |k| crate::app::short_name(k.name));
    let part = stem.split_once(" - ").map(|(p, _)| p.to_string()).filter(|p| p.starts_with("500-"));
    let size = format!("{} MB", id.size.div_ceil(1 << 20));
    let subtitle = [part.clone(), Some(size.clone())].into_iter().flatten().collect::<Vec<_>>().join("  ·  ");
    let key = format!("{:08x}", id.crc32);
    let mut details = vec![("System", "Zippity".to_string())];
    if let Some(p) = part {
        details.push(("Part number", p));
    }
    details.push(("Dump", if known.is_some() { "verified (known checksum)" } else { "not in Ribbit's list" }.into()));
    details.push(("Size", size));
    details.push(("CRC32", key.clone()));
    details.push(("File", path.display().to_string()));
    Game { path: path.to_path_buf(), family: Family::Zippity, title, subtitle, details, tags: Vec::new(), key, art: Art::Missing }
}

/// The games built into a Zippity (its system NAND), as a library entry
/// tagged "Internal".
fn describe_zippity_nand(path: &Path, known: Option<&ribbit_memory::known::KnownImage>, id: &ribbit_memory::ImageId) -> Game {
    // "Zippity (US) NAND" -> "US".
    let region = known.and_then(|k| k.name.split_once('(')).and_then(|(_, r)| r.split_once(')')).map(|(r, _)| r);
    let title = match region {
        Some(r) => format!("Zippity Built-in Games ({r})"),
        None => "Zippity Built-in Games".to_string(),
    };
    let key = format!("{:08x}", id.crc32);
    let details = vec![
        ("System", "Zippity".to_string()),
        ("Games", "built into the console (system NAND)".to_string()),
        ("Dump", if known.is_some() { "verified (known checksum)" } else { "not in Ribbit's list" }.into()),
        ("Size", format!("{} MB", id.size.div_ceil(1 << 20))),
        ("CRC32", key.clone()),
        ("File", path.display().to_string()),
    ];
    Game {
        path: path.to_path_buf(),
        family: Family::Zippity,
        title,
        subtitle: "Mickey Mouse Clubhouse, Pooh, Little Einsteins and more".into(),
        details,
        tags: vec!["Internal".into()],
        key,
        art: Art::Missing,
    }
}

/// Whether a BIOS has games of its own, played with no cartridge in: the
/// original Leapster's 2.x-generation BaseROMs (US, UK, German v2.1 and the
/// Spanish one) have a couple of Learning with Leap games and the
/// Educational Game Tours (a player's report); the Universal v1.5 has none,
/// nor does the Leapster TV's. The Leapster 2's has its main menu
/// (drawing and other activities). The L-MAX's: not known yet.
pub fn bios_has_games(hw: Hardware, name: &str) -> bool {
    match hw {
        Hardware::Leapster => !name.contains("v1.5"),
        Hardware::Leapster2 => true,
        _ => false,
    }
}

/// The library card for a BIOS's built-in games (played with no cartridge).
fn describe_builtin(path: &Path, bios: &BiosFile) -> Game {
    let crc = ribbit_core::read_image(path).ok().map(|d| ribbit_memory::ImageId::of(&d).crc32).unwrap_or_default();
    let key = format!("bios-{crc:08x}");
    let (family, title, subtitle) = match bios.hardware {
        Hardware::Leapster2 => (
            Family::Leapster2,
            format!("Leapster 2 Main Menu ({})", bios.short_name()),
            "Drawing and other activities",
        ),
        _ => (
            Family::Leapster,
            format!("Leapster Built-in Games ({})", bios.short_name()),
            "Art, a rabbit word game, Game Tours",
        ),
    };
    Game {
        path: path.to_path_buf(),
        family,
        title,
        subtitle: subtitle.into(),
        details: vec![
            ("System", bios.hardware.name().to_string()),
            ("Games", "built into the BIOS (played with no cartridge in)".to_string()),
            ("BIOS", bios.name.clone()),
            ("File", path.display().to_string()),
        ],
        tags: vec!["Internal".into()],
        key,
        art: Art::Missing,
    }
}

fn describe(path: &Path, cart: &Cartridge, key: String, art: Art) -> Game {
    let known = cart.known();
    let header_title = cart.header_text(0x0b);
    let part = cart.header_text(0x0a);
    let built = cart.header_text(0x0f);
    let stem = path.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    // Known dumps by name; otherwise the file name, which players usually
    // keep tidy (the header's title can be generic, e.g. "Included Title").
    let named = known
        .map(|k| crate::app::short_name(k.name))
        .or_else(|| (!stem.is_empty()).then(|| stem.clone()))
        .or(header_title.clone())
        .unwrap_or_default();
    // Its tags come off the end; the region stays on (the card shows it).
    let name = name_tags(&named);
    let title = match &name.region {
        Some(r) => format!("{} ({r})", name.title),
        None => name.title.clone(),
    };
    let version = name.version.clone().or_else(|| cart.header_text(0x07).as_deref().and_then(header_version));
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
    if let Some(v) = &version {
        details.push(("Version", v.clone()));
    }
    if let Some(b) = &built {
        details.push(("Built", b.clone()));
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
    let family = if known.is_some_and(|k| LEAPSTER2_ONLY.contains(&k.crc32)) { Family::Leapster2 } else { Family::Leapster };
    let mut tags: Vec<std::borrow::Cow<'static, str>> = name.extras.into_iter().map(Into::into).collect();
    let prototype = name.prototype || (known.is_none() && is_prototype(&stem));
    if prototype {
        tags.push("Prototype".into());
    }
    if let Some(v) = version {
        tags.push(v.into());
    }
    // A prototype's build date tells one build from another: the header's,
    // else the file name's, else a compile stamp found in the program.
    if prototype
        && let Some(date) = built
            .as_deref()
            .and_then(pretty_date)
            .or_else(|| name.date.as_deref().and_then(pretty_date))
            .or_else(|| build_date(cart.rom()))
    {
        tags.push(date.into());
    }
    Game { path: path.to_path_buf(), family, title, subtitle, details, tags, key, art }
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
            if self.m.run_cycles(chunk - self.m.cpu.cycles, &none).stop != ribbit_core::StopReason::StepLimit {
                return false;
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
                c.apply(&p);
            }
            // Every frame, as the app does: the canvas learns where the LCD
            // changed outside its renders (games drawn by their own code),
            // and shows the LCD there.
            c.update_mask(&self.frame[..]);
        }
    }

    /// The picture now: HD when available (kept true to the LCD).
    fn shot(&mut self) -> Shot {
        self.drain();
        match self.canvas.as_mut() {
            Some(c) => Shot { size: c.size, pixels: c.composed(&self.frame[..]) },
            None => Shot { size: WIDTH, pixels: self.frame.to_vec() },
        }
    }
}

fn make_art(src: &ArtSource, cart: Cartridge) -> Option<Shot> {
    let title_card = title_card_ms(&format!("{:08x}", cart.id().crc32));
    let fw = ribbit_core::load_firmware(&src.bios).ok()?;
    // Straps bit 14 set: the BIOS takes the saved calibration (no targets).
    let mut m = Machine::with_straps(fw, Some(cart), 0x63FF_FFFF);
    m.record_trace_tail = false;
    m.io.eeprom.load(ART_PROFILE);
    m.enable_experimental_mame_touch(Default::default());
    let canvas = m.enable_hd_capture().then(|| {
        let mut c = ribbit_core::hd::HdCanvas::new(m.hd.as_ref().unwrap().hooks);
        // Text from the fonts' outlines, as HD shows it: the fonts are
        // searched in the background as the library is; waited for a while.
        for _ in 0..300 {
            if crate::device_fonts::get().is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        c.fonts = crate::device_fonts::get();
        c
    });
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

/// How long a BIOS runs for its built-in games' picture: past its warning,
/// logo and region screens (region pickers move on by themselves after
/// about 30 s) to its menu.
const BIOS_ART_MS: u64 = 45_000;

/// Art for a BIOS's built-in games: the BIOS started with no cartridge (as
/// a console with its slot empty); the last screen that holds still and is
/// colourful enough (its menu, after the warning and logo screens), else
/// its most colourful one.
fn make_bios_art(bios: &Path) -> Option<Shot> {
    let fw = ribbit_core::load_firmware(bios).ok()?;
    // Straps bit 14 set: the BIOS takes the saved calibration (no targets).
    let mut m = Machine::with_straps(fw, None, 0x63FF_FFFF);
    m.record_trace_tail = false;
    m.io.eeprom.load(ART_PROFILE);
    let mut cap = Capture { m, canvas: None, frame: Box::new([0u32; WIDTH * HEIGHT]) };
    // (The latest still, colourful screen; and the most colourful of all.)
    let mut latest: Option<Shot> = None;
    let mut richest: Option<(usize, Shot)> = None;
    let mut last = cap.frame.clone();
    for i in 1..=BIOS_ART_MS / ART_SAMPLE_MS {
        if !cap.run_to(i * ART_SAMPLE_MS) {
            break;
        }
        cap.drain();
        let still = *cap.frame == *last;
        last.copy_from_slice(&cap.frame[..]);
        let colours = colourfulness(&cap.frame);
        if still && colours >= 48 {
            latest = Some(cap.shot());
        }
        if richest.as_ref().is_none_or(|b| colours > b.0) {
            richest = Some((colours, cap.shot()));
        }
    }
    // A blank or near-blank screen is no art.
    latest.or(richest.filter(|b| b.0 >= 16).map(|b| b.1))
}

/// Art for the Zippity's built-in games: its start-up, run to the Zippity
/// logo, the middle 480x480 of the 640x480 picture.
fn make_zippity_art(nand: &Path) -> Option<Shot> {
    use ribbit_zippity::{SCREEN_H, SCREEN_W};
    /// Frames from power-on to the Zippity logo.
    // (Re-timed 2026-10-16: the logo shows about frames 840-870 since the
    // Zippity's frame timing; 330 is now its photosensitivity warning.)
    const LOGO_FRAMES: usize = 855;
    let mut z = ribbit_zippity::Zippity::new(ribbit_core::read_image(nand).ok()?);
    for _ in 0..LOGO_FRAMES {
        z.run_frame();
        z.take_audio();
    }
    let mut frame = vec![0u32; SCREEN_W * SCREEN_H];
    z.render(&mut frame);
    let x0 = (SCREEN_W - SCREEN_H) / 2;
    let pixels = (0..SCREEN_H).flat_map(|y| frame[y * SCREEN_W + x0..y * SCREEN_W + x0 + SCREEN_H].to_vec()).collect();
    Some(Shot { size: SCREEN_H, pixels })
}

/// Art for a Zippity cartridge: the console started with it in the slot, up
/// to 40 s; the last screen that holds still and is colourful (its title,
/// after the console's warning and logo), else the most colourful one; the
/// middle 480x480 of the picture.
fn make_zippity_cart_art(nand: &Path, cart: &Path) -> Option<Shot> {
    use ribbit_zippity::{SCREEN_H, SCREEN_W};
    const FRAMES: usize = 2400;
    const EVERY: usize = 60;
    let mut z = ribbit_zippity::Zippity::new(ribbit_core::read_image(nand).ok()?);
    z.insert_cartridge(ribbit_core::read_image(cart).ok()?);
    let mut frame = vec![0u32; SCREEN_W * SCREEN_H];
    let mut last = frame.clone();
    let (mut latest, mut richest): (Option<Vec<u32>>, Option<(usize, Vec<u32>)>) = (None, None);
    for i in 1..=FRAMES {
        z.run_frame();
        z.take_audio();
        if i % EVERY != 0 {
            continue;
        }
        z.render(&mut frame);
        let colours = frame.iter().map(|p| p & 0x00FF_FFFF).collect::<HashSet<_>>().len();
        if frame == last && colours >= 64 {
            latest = Some(frame.clone());
        }
        if richest.as_ref().is_none_or(|b| colours > b.0) {
            richest = Some((colours, frame.clone()));
        }
        last.copy_from_slice(&frame);
    }
    let best = latest.or(richest.filter(|b| b.0 >= 16).map(|b| b.1))?;
    let x0 = (SCREEN_W - SCREEN_H) / 2;
    let pixels = (0..SCREEN_H).flat_map(|y| best[y * SCREEN_W + x0..y * SCREEN_W + x0 + SCREEN_H].to_vec()).collect();
    Some(Shot { size: SCREEN_H, pixels })
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
    fn the_bios_chosen_with_open_bios_counts_for_its_console() {
        let mut lib = Library::new(Path::new("."));
        assert_eq!(lib.consoles(), [Hardware::Leapster], "nothing yet: the original Leapster");
        // A Leapster 2 BIOS chosen with File > Open BIOS, none in the folders.
        let file = |hardware, path: &str, name: &str| BiosFile { hardware, path: PathBuf::from(path), name: name.into() };
        lib.opened = vec![file(Hardware::Leapster2, "l2.bin", "0.6")];
        assert_eq!(lib.consoles(), [Hardware::Leapster2]);
        assert_eq!(lib.selected_console("Leapster"), Hardware::Leapster2);
        assert_eq!(lib.bios_for(Hardware::Leapster2), Some(Path::new("l2.bin")));
        assert_eq!(lib.bios_for(Hardware::Leapster), None);
        // An original Leapster BIOS in a games folder as well: both, as chosen.
        lib.bios.push(file(Hardware::Leapster, "v21.bin", "US v2.1 (152-11265)"));
        assert_eq!(lib.consoles(), [Hardware::Leapster, Hardware::Leapster2]);
        assert_eq!(lib.selected_console("Leapster"), Hardware::Leapster);
        assert_eq!(lib.selected_console("Leapster 2"), Hardware::Leapster2);
        // Several Leapster 2 BIOSes: the picked one plays, while it is there.
        lib.bios.push(file(Hardware::Leapster2, "m9.bin", "CIP3 m9"));
        assert_eq!(lib.bios_list(Hardware::Leapster2).len(), 2);
        assert_eq!(lib.bios_for(Hardware::Leapster2), Some(Path::new("l2.bin")), "Open BIOS's first");
        lib.set_picked_bios(&[("Leapster 2".to_string(), PathBuf::from("m9.bin"))].into_iter().collect());
        assert_eq!(lib.bios_for(Hardware::Leapster2), Some(Path::new("m9.bin")));
        lib.set_picked_bios(&[("Leapster 2".to_string(), PathBuf::from("gone.bin"))].into_iter().collect());
        assert_eq!(lib.bios_for(Hardware::Leapster2), Some(Path::new("l2.bin")), "a missing pick is ignored");
        assert_eq!(bios_name("Leapster BaseROM US v2.1 (152-11265)", Hardware::Leapster), "US v2.1 (152-11265)");
        assert_eq!(file(Hardware::Leapster, "x", "US v2.1 (152-11265)").short_name(), "US v2.1");
    }

    #[test]
    fn every_bios_opened_stays_listed() {
        // Two Leapster BIOSes opened one after the other (newest first), and
        // one in a games folder: all three listed, the newest in use.
        let mut lib = Library::new(Path::new("."));
        let file = |path: &str, name: &str| BiosFile { hardware: Hardware::Leapster, path: PathBuf::from(path), name: name.into() };
        lib.opened = vec![file("uk.bin", "UK"), file("us.bin", "US")];
        lib.bios.push(file("universal.bin", "Universal"));
        lib.bios.push(file("us.bin", "US"));
        let paths: Vec<&Path> = lib.bios_list(Hardware::Leapster).iter().map(|b| b.path.as_path()).collect();
        assert_eq!(paths, [Path::new("uk.bin"), Path::new("us.bin"), Path::new("universal.bin")]);
        assert_eq!(lib.bios_for(Hardware::Leapster), Some(Path::new("uk.bin")));
    }

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
    fn prototypes_by_name() {
        assert!(is_prototype("Wall-E December 20th 2007 prototype"));
        assert!(is_prototype("Counting on Zero (prototype, repaired)"));
        assert!(is_prototype("ROX Alpha 1.15r2"));
        assert!(is_prototype("Disney Princess Enchanted Learning (1.8 Beta)"));
        assert!(is_prototype("The Incredibles - Animation Sample 1"));
        assert!(!is_prototype("Wall-E (USA)"));
        assert!(!is_prototype("Alphabet Park"));
    }

    #[test]
    fn file_name_tags_read_from_the_right() {
        let t = name_tags("Cars - Supercharged (USA) (Leapster2) (Proto) (Alpha 002) (2008-04-23)");
        assert_eq!(t.title, "Cars - Supercharged");
        assert_eq!(t.region.as_deref(), Some("USA"));
        assert_eq!(t.extras, ["Leapster2"]);
        assert!(t.prototype);
        assert_eq!(t.version.as_deref(), Some("Alpha 002"));
        assert_eq!(t.date.as_deref(), Some("2008-04-23"));
        let t = name_tags("NASCAR (USA) (L-MAX) (v1.0) (2006-02-24)");
        assert_eq!((t.title.as_str(), t.extras.len(), t.prototype, t.version.as_deref()), ("NASCAR", 1, false, Some("v1.0")));
        // A title with parentheses of its own, and names in other formats.
        assert_eq!(name_tags("Scooby-Doo (Et les nombres fous) (France) (v1.0)").title, "Scooby-Doo (Et les nombres fous)");
        assert_eq!(name_tags("Bratz World (500-11201-A, USA)").title, "Bratz World (500-11201-A, USA)");
        assert_eq!(name_tags("Counting on Zero (prototype, repaired)").region, None);
        assert_eq!(name_tags("Wall-E (USA)").title, "Wall-E");
    }

    #[test]
    fn versions_and_dates_normalised() {
        assert_eq!(header_version("v1.0").as_deref(), Some("v1.0"));
        assert_eq!(header_version("A03, v1.0").as_deref(), Some("v1.0"));
        assert_eq!(header_version("v 1.1").as_deref(), Some("v1.1"));
        assert_eq!(header_version("1").as_deref(), Some("v1.0"));
        assert_eq!(header_version("v1.0 - Initial Release JBM3").as_deref(), Some("v1.0"));
        assert_eq!(header_version("Example do not ship or release"), None);
        assert_eq!(pretty_date("2008-04-23").as_deref(), Some("Apr 23, 2008"));
        assert_eq!(pretty_date("Dec 20 2007 16:28:29").as_deref(), Some("Dec 20, 2007"));
        assert_eq!(pretty_date("Feb  9 2005").as_deref(), Some("Feb 9, 2005"));
        assert_eq!(pretty_date("v1.0"), None);
    }

    #[test]
    fn build_date_is_the_latest_compile_stamp() {
        let mut rom = vec![0u8; 256];
        rom[10..30].copy_from_slice(b"Feb  9 2005 10:00:00");
        rom[100..120].copy_from_slice(b"Apr 29 2005 13:50:19");
        rom[200..211].copy_from_slice(b"Dec 31 2009"); // no time: not a build stamp
        assert_eq!(build_date(&rom).as_deref(), Some("Apr 29, 2005"));
        assert_eq!(build_date(&[0; 64]), None);
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
        let carts: Vec<_> = files.iter().filter(|p| matches!(classify(p), Some(Found::Cart(_)))).collect();
        assert_eq!(carts, [&dir.join("sub").join("game.bin")]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
