//! Installed content: where a package's files go on the console's F: drive,
//! as LeapFrog Connect would put them, from the package's own `meta.inf`
//! (VERIFIED against the System's launcher, 0x215F0 and 0x21280: it looks in
//! `F:\Applications\`, `F:\Music\`, `F:\Videos\` and `F:\Trailers\`, the
//! built-in set in `Bundle` under each).
//!
//! A package arrives as a folder holding `meta.inf`: either `Bundle` (the
//! built-in apps, music or trailers) or one named after its package ID (an
//! add-on game pack). Its `Type` says which top folder it goes in.

use crate::package::Meta;

/// What kind of content a package is (`meta.inf` `Type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Application,
    Music,
    Video,
    Trailer,
}

impl Kind {
    pub fn parse(t: &str) -> Option<Self> {
        match t.to_ascii_lowercase().as_str() {
            "application" => Some(Kind::Application),
            "music" => Some(Kind::Music),
            "video" => Some(Kind::Video),
            "trailer" => Some(Kind::Trailer),
            _ => None,
        }
    }

    /// The folder on F: it is installed under.
    pub fn top(self) -> &'static str {
        match self {
            Kind::Application => "Applications",
            Kind::Music => "Music",
            Kind::Video => "Videos",
            Kind::Trailer => "Trailers",
        }
    }
}

/// A package found in an archive or folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pack {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    pub locale: String,
    /// The folder holding its `meta.inf`, as found ("/" between names, no
    /// trailing "/").
    pub source: String,
    /// Where that folder goes on F: ("Applications/Bundle",
    /// "Applications/PCKT-0x0050000E-000000"...).
    pub destination: String,
}

/// The folder part of a path ("/" or "\" between names).
fn parent(path: &str) -> &str {
    path.rfind(['/', '\\']).map_or("", |i| &path[..i])
}

fn last(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// The package described by the `meta.inf` at `path` with contents `text`,
/// if it is installable content.
pub fn pack(path: &str, text: &str) -> Option<Pack> {
    let meta = Meta::parse(text.trim_start_matches('\u{feff}'));
    let kind = Kind::parse(meta.get("Type")?)?;
    let source = parent(path).replace('\\', "/");
    let folder = last(&source);
    if folder.is_empty() {
        return None;
    }
    Some(Pack {
        id: meta.get("PackageID").unwrap_or(folder).to_string(),
        name: meta.get("Name").unwrap_or(folder).to_string(),
        kind,
        locale: meta.get("Locale").unwrap_or("").to_ascii_lowercase(),
        destination: format!("{}/{folder}", kind.top()),
        source,
    })
}

/// Where the file at `path` goes on F:, if it belongs to one of `packs`.
pub fn destination_of(packs: &[Pack], path: &str) -> Option<String> {
    let path = path.replace('\\', "/");
    packs.iter().find_map(|p| {
        let rest = path.strip_prefix(&p.source)?.strip_prefix('/')?;
        (!rest.is_empty() && !rest.ends_with('/')).then(|| format!("{}/{rest}", p.destination))
    })
}

/// The packs to install for one console: those of `locale` (a console has
/// one region), the built-in sets once each.
pub fn for_locale(packs: &[Pack], locale: &str) -> Vec<Pack> {
    let mut out: Vec<Pack> = Vec::new();
    for p in packs.iter().filter(|p| p.locale.eq_ignore_ascii_case(locale)) {
        if !out.iter().any(|o| o.destination.eq_ignore_ascii_case(&p.destination)) {
            out.push(p.clone());
        }
    }
    out
}

/// The locales present, sorted by name.
pub fn locales(packs: &[Pack]) -> Vec<String> {
    let mut l: Vec<String> = packs.iter().map(|p| p.locale.clone()).filter(|l| !l.is_empty()).collect();
    l.sort();
    l.dedup();
    l
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUNDLE: &str = "MetaVersion=\"1.0\"\r\nType=\"Application\"\r\nPackageID=\"PCKT-0x00500001-000000\"\r\nLocale=\"en-us\"\r\nName=\"Included App Bundle\"\r\n";

    #[test]
    fn packages_go_where_the_launcher_looks() {
        let b = pack(r"Games\PCKT-0x00500001-000000\Bundle\meta.inf", BUNDLE).unwrap();
        assert_eq!(
            (b.kind, b.destination.as_str(), b.source.as_str()),
            (Kind::Application, "Applications/Bundle", "Games/PCKT-0x00500001-000000/Bundle")
        );
        let music =
            pack("G/PCKT-0x00500003-000000/Bundle/meta.inf", "\u{feff}Type=\"Music\"\nLocale=\"en-us\"").unwrap();
        assert_eq!(music.destination, "Music/Bundle");
        let trailer = pack("G/x/Bundle/meta.inf", "Type=\"Trailer\"\nLocale=\"en-US\"").unwrap();
        assert_eq!((trailer.destination.as_str(), trailer.locale.as_str()), ("Trailers/Bundle", "en-us"));
        let game =
            pack("G/PCKT-0x0050000E-000000/PCKT-0x0050000E-000000/meta.inf", "Type=\"Application\"\nLocale=\"en-us\"")
                .unwrap();
        assert_eq!(game.destination, "Applications/PCKT-0x0050000E-000000");
        assert_eq!(pack("G/a/meta.inf", "Type=\"Firmware\""), None, "system packages are not content");
        let packs = [b, game];
        assert_eq!(
            destination_of(&packs, r"Games\PCKT-0x00500001-000000\Bundle\fapp\Bunny.bin").as_deref(),
            Some("Applications/Bundle/fapp/Bunny.bin")
        );
        assert_eq!(destination_of(&packs, "Games/PCKT-0x00500001-000000/Bundle/US/"), None, "folders");
        assert_eq!(destination_of(&packs, "Games/other.txt"), None);
    }

    #[test]
    fn one_region_is_chosen() {
        let p = |path: &str, locale: &str| pack(path, &format!("Type=\"Application\"\nLocale=\"{locale}\"")).unwrap();
        let all = [
            p("a/Bundle/meta.inf", "en-us"),
            p("b/Bundle/meta.inf", "en-gb"),
            p("c/PCKT-1/meta.inf", "en-us"),
            p("d/Bundle/meta.inf", "en-us"),
        ];
        let us = for_locale(&all, "en-us");
        assert_eq!(
            us.iter().map(|p| p.source.as_str()).collect::<Vec<_>>(),
            ["a/Bundle", "c/PCKT-1"],
            "one Bundle per kind"
        );
        assert_eq!(locales(&all), ["en-gb", "en-us"]);
    }
}
