//! Update check: asks GitHub for Ribbit's releases (in the background, once
//! at start-up when the setting is on, or from Help → Check for updates),
//! says whether a newer one is out and shows every release's patch notes.
//! Nothing is downloaded or installed: "Download" opens the release page.

use std::sync::{Arc, Mutex};

/// The repository releases are published from.
pub const REPO: &str = "MotifMixes/ribbit";
const API: &str = "https://api.github.com/repos/MotifMixes/ribbit/releases?per_page=30";

/// The version running, from Cargo.toml.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    /// "v1.1.0".
    pub tag: String,
    /// "Ribbit 1.1.0".
    pub name: String,
    /// The release notes (GitHub Markdown).
    pub notes: String,
    /// "2026-10-17".
    pub date: String,
    /// The release page.
    pub url: String,
    pub prerelease: bool,
    /// Parsed from the tag (None: not a version tag).
    pub version: Option<Version>,
}

impl Release {
    /// "1.2.0" (the tag when it is not a version).
    pub fn version_text(&self) -> String {
        self.version.map_or_else(|| self.tag.clone(), |(a, b, c)| format!("{a}.{b}.{c}"))
    }
}

/// major.minor.patch.
pub type Version = (u32, u32, u32);

#[derive(Debug, Clone, Default)]
pub enum Status {
    #[default]
    NotChecked,
    Checking,
    /// Releases, newest first.
    Done(Vec<Release>),
    Failed(String),
}

/// The check's state, shared with its thread.
#[derive(Default)]
pub struct Updates {
    status: Arc<Mutex<Status>>,
}

impl Updates {
    /// Starts a check in the background (unless one is running); `ctx`
    /// repaints when it ends.
    pub fn check(&self, ctx: &egui::Context) {
        {
            let mut s = self.status.lock().unwrap();
            if matches!(*s, Status::Checking) {
                return;
            }
            *s = Status::Checking;
        }
        let status = Arc::clone(&self.status);
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new().name("ribbit-updates".into()).spawn(move || {
            let result = match fetch() {
                Ok(releases) => Status::Done(releases),
                Err(e) => {
                    tracing::info!(target: "ribbit::updates", "update check failed: {e}");
                    Status::Failed(e)
                }
            };
            *status.lock().unwrap() = result;
            ctx.request_repaint();
        });
        if let Err(e) = spawned {
            *self.status.lock().unwrap() = Status::Failed(e.to_string());
        }
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    pub fn checked(&self) -> bool {
        !matches!(*self.status.lock().unwrap(), Status::NotChecked)
    }

    /// The newest release later than this version (pre-releases left out).
    pub fn newer(&self) -> Option<Release> {
        match &*self.status.lock().unwrap() {
            Status::Done(releases) => newest_after(releases, parse_version(CURRENT)?).cloned(),
            _ => None,
        }
    }
}

fn newest_after(releases: &[Release], current: Version) -> Option<&Release> {
    releases.iter().filter(|r| !r.prerelease && r.version.is_some_and(|v| v > current)).max_by_key(|r| r.version)
}

/// "v1.2.3", "1.2", "Ribbit-1.2.3-beta" -> (1, 2, 3) etc.
pub fn parse_version(text: &str) -> Option<Version> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let mut parts = text[start..].split(|c: char| !c.is_ascii_digit() && c != '.').next()?.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().map_or(Some(0), |p| p.parse().ok())?;
    let patch = parts.next().map_or(Some(0), |p| p.parse().ok())?;
    Some((major, minor, patch))
}

#[cfg(not(target_os = "android"))]
fn fetch() -> Result<Vec<Release>, String> {
    let agent: ureq::Agent =
        ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(15))).build().into();
    let body = agent
        .get(API)
        .header("User-Agent", concat!("Ribbit/", env!("CARGO_PKG_VERSION")))
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| match e {
            ureq::Error::StatusCode(404) => "no releases found (is the repository public?)".to_string(),
            ureq::Error::StatusCode(403 | 429) => "GitHub's rate limit was reached; try again later".to_string(),
            e => e.to_string(),
        })?
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    parse_releases(&body)
}

#[cfg(target_os = "android")]
fn fetch() -> Result<Vec<Release>, String> {
    Err("not available in the Android app yet".to_string())
}

/// GitHub's release list (JSON) -> releases, newest first, drafts left out.
fn parse_releases(json: &str) -> Result<Vec<Release>, String> {
    let list: Vec<serde_json::Value> = serde_json::from_str(json).map_err(|e| format!("unexpected reply: {e}"))?;
    let text = |v: &serde_json::Value, k: &str| v.get(k).and_then(|s| s.as_str()).unwrap_or_default().to_string();
    let mut releases: Vec<Release> = list
        .iter()
        .filter(|r| !r.get("draft").and_then(|d| d.as_bool()).unwrap_or(false))
        .map(|r| {
            let tag = text(r, "tag_name");
            let name = text(r, "name");
            Release {
                version: parse_version(&tag),
                name: if name.is_empty() { tag.clone() } else { name },
                notes: text(r, "body").replace("\r\n", "\n"),
                date: text(r, "published_at").chars().take(10).collect(),
                url: text(r, "html_url"),
                prerelease: r.get("prerelease").and_then(|p| p.as_bool()).unwrap_or(false),
                tag,
            }
        })
        .collect();
    releases.sort_by(|a, b| b.version.cmp(&a.version).then_with(|| b.date.cmp(&a.date)));
    Ok(releases)
}

/// "2026-10-17" -> "17 October 2026".
pub fn long_date(date: &str) -> String {
    const MONTHS: [&str; 12] = [
        "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November",
        "December",
    ];
    let mut parts = date.split('-');
    match (parts.next(), parts.next().and_then(|m| m.parse::<usize>().ok()), parts.next()) {
        (Some(y), Some(m @ 1..=12), Some(d)) => format!("{} {} {y}", d.trim_start_matches('0'), MONTHS[m - 1]),
        _ => date.to_string(),
    }
}

/// Draws release notes: headings, bullets and paragraphs of GitHub
/// Markdown, with emphasis, code marks and link targets dropped.
pub fn notes_ui(ui: &mut egui::Ui, notes: &str) {
    if notes.trim().is_empty() {
        ui.label(egui::RichText::new("No notes for this release.").weak());
        return;
    }
    for line in notes.lines() {
        let line = line.trim_end();
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            ui.add_space(4.0);
        } else if let Some(h) = trimmed.strip_prefix('#') {
            ui.label(egui::RichText::new(plain(h.trim_start_matches('#').trim())).strong().size(15.0));
        } else if let Some(item) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            let indent = (line.len() - trimmed.len()) as f32 * 4.0;
            ui.horizontal_top(|ui| {
                ui.add_space(6.0 + indent);
                ui.label("•");
                ui.add(egui::Label::new(plain(item)).wrap());
            });
        } else {
            ui.add(egui::Label::new(plain(trimmed)).wrap());
        }
    }
}

/// Markdown inline marks removed: **bold**, `code`, [text](url) -> text.
fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find("](").map(|c| open + c) else { break };
        let Some(end) = rest[close..].find(')').map(|e| close + e) else { break };
        out.push_str(&rest[..open]);
        out.push_str(&rest[open + 1..close]);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    // (The app's fonts have no arrow: "File → Open" reads "File > Open",
    // as in Ribbit's own messages.)
    out.replace("**", "").replace("__", "").replace('`', "").replace('→', ">")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_from_tags() {
        assert_eq!(parse_version("v1.1.0"), Some((1, 1, 0)));
        assert_eq!(parse_version("1.2"), Some((1, 2, 0)));
        assert_eq!(parse_version("Ribbit-2.0.3-beta"), Some((2, 0, 3)));
        assert_eq!(parse_version("nightly"), None);
        assert!(parse_version(CURRENT).is_some());
    }

    #[test]
    fn releases_parse_and_the_newest_later_one_is_found() {
        let json = r#"[
            {"tag_name":"v1.0.0","name":"Ribbit 1.0.0","body":"First\r\n- one","published_at":"2026-10-15T06:31:20Z","html_url":"u0","draft":false,"prerelease":false},
            {"tag_name":"v1.2.0","name":"","body":"","published_at":"2026-10-20T00:00:00Z","html_url":"u2","draft":false,"prerelease":false},
            {"tag_name":"v1.3.0","name":"Beta","body":"","published_at":"2026-10-21T00:00:00Z","html_url":"u3","draft":false,"prerelease":true},
            {"tag_name":"v9.0.0","name":"Draft","body":"","published_at":"","html_url":"","draft":true,"prerelease":false}
        ]"#;
        let r = parse_releases(json).unwrap();
        assert_eq!(r.iter().map(|r| r.tag.as_str()).collect::<Vec<_>>(), ["v1.3.0", "v1.2.0", "v1.0.0"]);
        assert_eq!(r[1].name, "v1.2.0");
        assert_eq!(r[2].notes, "First\n- one");
        assert_eq!(r[2].date, "2026-10-15");
        assert_eq!(newest_after(&r, (1, 1, 0)).map(|r| r.tag.as_str()), Some("v1.2.0"));
        assert_eq!(newest_after(&r, (1, 2, 0)), None);
    }

    #[test]
    fn markdown_marks_are_dropped() {
        assert_eq!(plain("**Bold** and `code`, [a link](https://x) end"), "Bold and code, a link end");
        assert_eq!(plain("File → Open BIOS"), "File > Open BIOS");
        assert_eq!(long_date("2026-10-07"), "7 October 2026");
    }
}
