//! The words on a LeapPad book's pages, made on this PC from the player's own
//! cartridge (cartridges hold no text, and the books' text is not Ribbit's to
//! ship): a background job records what each spot says with a second pad
//! (ribbit_leappad::words::Recorder) and transcribes it with Whisper
//! (ribbit_speech), page by page, the page on screen first. The results are
//! kept in the data folder, so a book is only transcribed once.
//!
//! Not laggy by design: the job runs in a helper process (this program
//! started again with `--leappad-words`, `worker_main`) at idle priority, on
//! one thread (two on PCs with eight or more). In Ribbit's own process, even
//! at idle priority, it held the whole process up on a 4-thread PC (the
//! LeapPad thread and the sound output waited up to 130 ms: the sound
//! clicked); in a process of its own the same work left the game alone. And a page's printed words come first (its sentence buttons,
//! about 9 s a page here), its pictures' remarks after.

use ribbit_leappad::book::Book;
use ribbit_leappad::words::{self, Recorder};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::OsString;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};

/// The speech model (English-only Whisper tiny: 150 MB; it agreed with the
/// base model on 59 of 60 words of At the Shore, 1.7 times faster).
pub const MODEL: &str = "whisper-tiny.en";
const MODEL_URL: &str = "https://huggingface.co/openai/whisper-tiny.en/resolve/main/";
/// The model's size, for the download button.
pub const MODEL_MB: u64 = 153;

pub fn model_dir(data: &Path) -> PathBuf {
    data.join("speech").join(MODEL)
}

pub fn model_ready(data: &Path) -> bool {
    ribbit_speech::MODEL_FILES.iter().all(|f| model_dir(data).join(f).is_file())
}

/// Where a cartridge's words are kept (by its contents, so renaming the
/// file keeps them).
pub fn cache_path(data: &Path, cart: &[u8]) -> PathBuf {
    data.join("leappad-words").join(format!("{:08x}-{:x}.tsv", crc32fast::hash(cart), cart.len()))
}

/// A cache file: each spread's words-file lines, and which spreads are done
/// (its first line, "# done 0 1 2").
pub fn read_cache(path: &Path) -> (BTreeMap<usize, String>, BTreeSet<usize>) {
    let mut lines: BTreeMap<usize, String> = BTreeMap::new();
    let mut done = BTreeSet::new();
    let Ok(text) = std::fs::read_to_string(path) else { return (lines, done) };
    for line in text.lines() {
        if let Some(list) = line.strip_prefix("# done") {
            done.extend(list.split_whitespace().filter_map(|n| n.parse::<usize>().ok()));
        } else if let Some(spread) = line.split('\t').next().and_then(|n| n.parse::<usize>().ok()) {
            let e = lines.entry(spread).or_default();
            e.push_str(line);
            e.push('\n');
        }
    }
    (lines, done)
}

fn write_cache(path: &Path, lines: &BTreeMap<usize, String>, done: &BTreeSet<usize>) {
    let mut text = format!("# done {}\n", done.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(" "));
    lines.values().for_each(|l| text.push_str(l));
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")));
    if let Err(e) = std::fs::write(path, text) {
        tracing::warn!(target: "ribbit::leappad", "keeping the words: {e}");
    }
}

/// What the job reports.
pub enum Msg {
    /// A spread's words so far (replacing its earlier ones).
    Spread(usize, String),
    Status(String),
    Failed(String),
    Done,
}

/// The running job (a helper process). Dropping it ends the helper.
pub struct Job {
    pub rx: mpsc::Receiver<Msg>,
    /// The spread on screen, done next.
    pub focus: Arc<AtomicUsize>,
    quit: Arc<AtomicBool>,
    child: Arc<Mutex<Option<std::process::Child>>>,
    /// The cartridge, base ROM and EEPROM handed to the helper.
    files: PathBuf,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::Relaxed);
        if let Some(mut c) = self.child.lock().unwrap().take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        let _ = std::fs::remove_dir_all(&self.files);
    }
}

/// The helper's command-line switch (`worker_main`).
pub const WORKER_ARG: &str = "--leappad-words";

/// Starts transcribing the spreads the cache doesn't have yet, in a helper
/// process.
pub fn start(bios: Vec<u8>, cart: Vec<u8>, eeprom: Option<Vec<u8>>, data: PathBuf) -> Job {
    let (tx, rx) = mpsc::channel();
    let focus = Arc::new(AtomicUsize::new(0));
    let quit = Arc::new(AtomicBool::new(false));
    static JOBS: AtomicUsize = AtomicUsize::new(0);
    let files = std::env::temp_dir().join(format!("ribbit-words-{}-{}", std::process::id(), JOBS.fetch_add(1, Ordering::Relaxed)));
    let job = Job { rx, focus: focus.clone(), quit: quit.clone(), child: Arc::new(Mutex::new(None)), files: files.clone() };
    if let Err(e) = spawn_helper(&files, &bios, &cart, eeprom.as_deref(), &data, &job.child, focus, quit, tx.clone()) {
        tracing::warn!(target: "ribbit::leappad", "starting the words: {e}");
        let _ = tx.send(Msg::Failed(format!("couldn't start: {e}")));
    }
    job
}

#[allow(clippy::too_many_arguments)]
fn spawn_helper(
    files: &Path,
    bios: &[u8],
    cart: &[u8],
    eeprom: Option<&[u8]>,
    data: &Path,
    child: &Arc<Mutex<Option<std::process::Child>>>,
    focus: Arc<AtomicUsize>,
    quit: Arc<AtomicBool>,
    tx: mpsc::Sender<Msg>,
) -> std::io::Result<()> {
    std::fs::create_dir_all(files)?;
    std::fs::write(files.join("bios"), bios)?;
    std::fs::write(files.join("cart"), cart)?;
    if let Some(e) = eeprom {
        std::fs::write(files.join("eeprom"), e)?;
    }
    let mut cmd = std::process::Command::new(std::env::current_exe()?);
    cmd.arg(WORKER_ARG)
        .arg(files)
        .arg(data)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // IDLE_PRIORITY_CLASS | CREATE_NO_WINDOW.
        cmd.creation_flags(0x0000_0040 | 0x0800_0000);
    }
    let mut c = cmd.spawn()?;
    let (stdin, stdout) = (c.stdin.take(), c.stdout.take());
    *child.lock().unwrap() = Some(c);
    tracing::info!(target: "ribbit::leappad", "transcribing the words (helper process)");
    // Its news, line by line.
    if let Some(out) = stdout {
        let q = quit.clone();
        std::thread::Builder::new().name("leappad-words-in".into()).spawn(move || {
            let mut finished = false;
            for line in std::io::BufReader::new(out).lines() {
                let Ok(line) = line else { break };
                let Some(msg) = decode(&line) else { continue };
                finished |= matches!(msg, Msg::Done | Msg::Failed(_));
                if tx.send(msg).is_err() {
                    return;
                }
            }
            if !finished && !q.load(Ordering::Relaxed) {
                let _ = tx.send(Msg::Failed("the words helper stopped".into()));
            }
        })?;
    }
    // The page on screen, when it changes.
    if let Some(mut input) = stdin {
        std::thread::Builder::new().name("leappad-words-out".into()).spawn(move || {
            let mut sent = usize::MAX;
            while !quit.load(Ordering::Relaxed) {
                let f = focus.load(Ordering::Relaxed);
                if f != sent {
                    if writeln!(input, "FOCUS {f}").and_then(|_| input.flush()).is_err() {
                        break;
                    }
                    sent = f;
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        })?;
    }
    Ok(())
}

/// One line of the helper's output (text with its newlines escaped).
fn encode(msg: &Msg) -> String {
    let esc = |t: &str| t.replace('\\', "\\\\").replace('\n', "\\n").replace('\r', "\\r");
    match msg {
        Msg::Spread(s, text) => format!("SPREAD\t{s}\t{}", esc(text)),
        Msg::Status(t) => format!("STATUS\t{}", esc(t)),
        Msg::Failed(t) => format!("FAILED\t{}", esc(t)),
        Msg::Done => "DONE".into(),
    }
}

fn decode(line: &str) -> Option<Msg> {
    let unesc = |t: &str| {
        let mut out = String::with_capacity(t.len());
        let mut chars = t.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some(other) => out.push(other),
                None => {}
            }
        }
        out
    };
    let mut parts = line.splitn(3, '\t');
    match parts.next()? {
        "SPREAD" => Some(Msg::Spread(parts.next()?.parse().ok()?, unesc(parts.next().unwrap_or("")))),
        "STATUS" => Some(Msg::Status(unesc(parts.next()?))),
        "FAILED" => Some(Msg::Failed(unesc(parts.next()?))),
        "DONE" => Some(Msg::Done),
        _ => None,
    }
}

/// The helper process (`WORKER_ARG <files> <data folder>`): transcribes the
/// book in `files` (bios, cart, eeprom), keeps the words in the data
/// folder's cache, and reports on stdout; reads the page on screen from
/// stdin, and ends when stdin closes (Ribbit closed the book or quit).
pub fn worker_main(args: &[OsString]) -> i32 {
    let (Some(files), Some(data)) = (args.first().map(PathBuf::from), args.get(1).map(PathBuf::from)) else {
        return 2;
    };
    let out = Mutex::new(std::io::stdout());
    let say = |msg: &Msg| {
        let mut o = out.lock().unwrap();
        let _ = writeln!(o, "{}", encode(msg)).and_then(|_| o.flush());
    };
    let (Ok(bios), Ok(cart)) = (std::fs::read(files.join("bios")), std::fs::read(files.join("cart"))) else {
        say(&Msg::Failed("the book's files are missing".into()));
        return 1;
    };
    let eeprom = std::fs::read(files.join("eeprom")).ok();
    let focus = Arc::new(AtomicUsize::new(0));
    let quit = Arc::new(AtomicBool::new(false));
    {
        let f = focus.clone();
        let _ = std::thread::Builder::new().name("focus".into()).spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                if let Some(n) = line.strip_prefix("FOCUS ").and_then(|n| n.trim().parse().ok()) {
                    f.store(n, Ordering::Relaxed);
                }
            }
            // Ribbit closed the book (or ended): nothing more to do.
            std::process::exit(0);
        });
    }
    let (lines, done) = read_cache(&cache_path(&data, &cart));
    // One thread unless the PC has eight or more (a low-end laptop's i3 has
    // four): the game keeps the rest.
    let threads = if crate::perf::usable_cpus() >= 8 { 2 } else { 1 };
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).start_handler(|_| crate::perf::idle_priority()).build();
    let (tx, rx) = mpsc::channel();
    let work = std::thread::spawn({
        let (f, q) = (focus.clone(), quit.clone());
        move || {
            crate::perf::idle_priority();
            let run = || run(bios, cart, eeprom, &data, lines, done, &f, &q, &tx);
            let result = match &pool {
                Ok(p) => p.install(run),
                Err(_) => run(),
            };
            let _ = tx.send(match result {
                Ok(()) => Msg::Done,
                Err(e) => Msg::Failed(e),
            });
        }
    });
    for msg in rx {
        say(&msg);
    }
    let _ = work.join();
    0
}

#[allow(clippy::too_many_arguments)]
fn run(
    bios: Vec<u8>,
    cart: Vec<u8>,
    eeprom: Option<Vec<u8>>,
    data: &Path,
    mut lines: BTreeMap<usize, String>,
    mut done: BTreeSet<usize>,
    focus: &AtomicUsize,
    quit: &AtomicBool,
    tx: &mpsc::Sender<Msg>,
) -> Result<(), String> {
    let cache = cache_path(data, &cart);
    let book = Book::from_cartridge(&cart, "words", None)?;
    let mut rec = ribbit_speech::Recognizer::load(&model_dir(data)).map_err(|e| format!("the speech model: {e:#}"))?;
    let mut recorder = Recorder::new(bios, cart, eeprom);
    let n = book.spreads.len();
    let t0 = std::time::Instant::now();
    loop {
        let want = focus.load(Ordering::Relaxed);
        let Some(s) = (!done.contains(&want) && want < n).then_some(want).or_else(|| (0..n).find(|s| !done.contains(s))) else {
            break;
        };
        let _ = tx.send(Msg::Status(format!("Loading words: page {} of {n}…", s + 1)));
        let clips: HashMap<usize, Vec<i16>> = recorder.record(&book, s, &[]).into_iter().map(|c| (c.region, c.audio)).collect();
        let regions = &book.spreads[s].regions;
        let mut heard: HashMap<usize, String> = HashMap::new();
        let mut prompted: HashMap<usize, String> = HashMap::new();
        let mut hear = |k: usize, prompt: Option<&str>| -> Result<Option<String>, String> {
            if quit.load(Ordering::Relaxed) {
                return Err("stopped".into());
            }
            match clips.get(&k) {
                Some(a) => rec.transcribe(&ribbit_speech::prepare_8k(a), prompt).map(Some).map_err(|e| format!("{e:#}")),
                None => Ok(None),
            }
        };
        let send = |heard: &HashMap<usize, String>, prompted: &HashMap<usize, String>| {
            let text = words::lines(&book, s, &words::assign(&book, s, heard, prompted));
            let _ = tx.send(Msg::Spread(s, text.clone()));
            text
        };
        // The sentences (and page numbers) first: the printed words follow
        // from them.
        let first: Vec<usize> = words::button_candidates(regions).into_iter().chain((0..regions.len()).filter(|&k| words::corner(&regions[k]))).collect();
        for k in first {
            if let Some(t) = hear(k, None)? {
                heard.insert(k, t);
            }
        }
        send(&heard, &prompted);
        let prompt = words::sentences(&book, s, &heard);
        for k in words::boxes_to_hear(&book, s, &heard) {
            if let Some(t) = hear(k, Some(&prompt))? {
                prompted.insert(k, t);
            }
        }
        send(&heard, &prompted);
        // Then what the pictures say (shown under the pen).
        let mut rest: Vec<usize> = clips.keys().copied().filter(|k| !heard.contains_key(k) && !prompted.contains_key(k)).collect();
        rest.sort_unstable();
        for k in rest {
            if let Some(t) = hear(k, None)? {
                heard.insert(k, t);
            }
        }
        lines.insert(s, send(&heard, &prompted));
        done.insert(s);
        write_cache(&cache, &lines, &done);
    }
    tracing::info!(target: "ribbit::leappad", "words for {} pages in {:.0} s", n, t0.elapsed().as_secs_f32());
    Ok(())
}

/// Downloads the speech model in the background; `progress` counts bytes.
pub struct Download {
    pub progress: Arc<AtomicU64>,
    pub result: mpsc::Receiver<Result<(), String>>,
}

pub fn download_model(data: &Path) -> Download {
    let progress = Arc::new(AtomicU64::new(0));
    let (tx, rx) = mpsc::channel();
    let (dir, p) = (model_dir(data), progress.clone());
    let _ = std::thread::Builder::new().name("speech-model".into()).spawn(move || {
        let _ = tx.send(fetch_model(&dir, &p));
    });
    Download { progress, result: rx }
}

#[cfg(not(target_os = "android"))]
fn fetch_model(dir: &Path, progress: &AtomicU64) -> Result<(), String> {
    use std::io::{Read, Write};
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_connect(Some(std::time::Duration::from_secs(15))).build().into();
    let mut total = 0u64;
    for name in ribbit_speech::MODEL_FILES {
        let response = agent
            .get(&format!("{MODEL_URL}{name}"))
            .header("User-Agent", concat!("Ribbit/", env!("CARGO_PKG_VERSION")))
            .call()
            .map_err(|e| format!("couldn't download the speech model: {e}"))?;
        let mut reader = response.into_body().into_with_config().limit(512 << 20).reader();
        // Written beside, then renamed: a cut download never looks complete.
        let part = dir.join(format!("{name}.part"));
        let mut file = std::fs::File::create(&part).map_err(|e| format!("{}: {e}", part.display()))?;
        let mut chunk = vec![0u8; 64 << 10];
        loop {
            let n = reader.read(&mut chunk).map_err(|e| format!("the download stopped: {e}"))?;
            if n == 0 {
                break;
            }
            file.write_all(&chunk[..n]).map_err(|e| format!("{}: {e}", part.display()))?;
            total += n as u64;
            progress.store(total, Ordering::Relaxed);
        }
        drop(file);
        std::fs::rename(&part, dir.join(name)).map_err(|e| format!("{}: {e}", part.display()))?;
    }
    Ok(())
}

#[cfg(target_os = "android")]
fn fetch_model(_dir: &Path, _progress: &AtomicU64) -> Result<(), String> {
    Err("not available in the Android app yet".into())
}

#[cfg(test)]
mod tests {
    /// The speech model's download, end to end (network; ~150 MB):
    /// `cargo test -p ribbit-frontend --release model_downloads -- --ignored`.
    #[test]
    #[ignore]
    #[cfg(not(target_os = "android"))]
    fn model_downloads() {
        let data = std::env::temp_dir().join(format!("ribbit-model-test-{}", std::process::id()));
        let d = super::download_model(&data);
        let result = d.result.recv().expect("the download thread answers");
        assert_eq!(result, Ok(()));
        assert!(super::model_ready(&data), "every model file is there");
        assert!(d.progress.load(std::sync::atomic::Ordering::Relaxed) > 100 << 20, "the whole model came");
        let _ = std::fs::remove_dir_all(&data);
    }
}
