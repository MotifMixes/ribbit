//! Ribbit's typeface: Fredoka (SIL Open Font License 1.1, see
//! THIRD-PARTY-NOTICES.txt), a rounded face in the spirit of the LeapFrog
//! lettering (LeapFrog's own logo type is theirs alone). Medium for all
//! text, Bold for titles; egui's own fonts stay behind them for anything
//! Fredoka lacks (symbols, emoji, other scripts).

use std::sync::Arc;

const TITLE: &str = "fredoka-bold";
const TEXT: &str = "fredoka-medium";

/// Makes Fredoka the font of all text in `ctx`, and adds the title family.
pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(TITLE.into(), Arc::new(egui::FontData::from_static(include_bytes!("fonts/Fredoka-Bold.ttf"))));
    fonts.font_data.insert(TEXT.into(), Arc::new(egui::FontData::from_static(include_bytes!("fonts/Fredoka-Medium.ttf"))));
    let fallbacks = fonts.families[&egui::FontFamily::Proportional].clone();
    for (family, first) in [(egui::FontFamily::Proportional, TEXT), (egui::FontFamily::Name(TITLE.into()), TITLE)] {
        let mut list = vec![first.to_string()];
        list.extend(fallbacks.iter().cloned());
        fonts.families.insert(family, list);
    }
    ctx.set_fonts(fonts);
}

/// The title font family.
pub fn title() -> egui::FontFamily {
    egui::FontFamily::Name(TITLE.into())
}
