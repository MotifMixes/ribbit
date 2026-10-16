//! Fonts beyond egui's defaults: Fredoka Bold (SIL Open Font License 1.1,
//! see THIRD-PARTY-NOTICES.txt) for titles, a rounded face in the spirit
//! of the LeapFrog lettering (LeapFrog's own logo type is theirs alone).

use std::sync::Arc;

const TITLE: &str = "fredoka-bold";

/// Adds the title font to `ctx`.
pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(TITLE.into(), Arc::new(egui::FontData::from_static(include_bytes!("fonts/Fredoka-Bold.ttf"))));
    // Titles fall back to the usual fonts for anything Fredoka lacks.
    let mut family = vec![TITLE.to_string()];
    family.extend(fonts.families[&egui::FontFamily::Proportional].iter().cloned());
    fonts.families.insert(egui::FontFamily::Name(TITLE.into()), family);
    ctx.set_fonts(fonts);
}

/// The title font family.
pub fn title() -> egui::FontFamily {
    egui::FontFamily::Name(TITLE.into())
}
