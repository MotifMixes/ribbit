//! The consoles' icons: each device drawn plain with egui shapes (original
//! art, no logos, wordmarks or pictures from firmware), all in one style:
//! the device seen from the front on a soft tile, a highlight along its top,
//! a dark glass screen, its own colours. One place for the library's cards,
//! its console chips and anything else that shows a console
//! (`paint`, `color`, `name`).
use ribbit_memory::known::Hardware;

type C = egui::Color32;

/// A console's own colour (its chip in the library).
pub fn color(hw: Hardware) -> C {
    match hw {
        Hardware::Leapster => C::from_rgb(150, 160, 175),
        Hardware::LeapsterTv => C::from_rgb(230, 140, 40),
        Hardware::LeapsterLmax => C::from_rgb(240, 120, 60),
        Hardware::Leapster2 => C::from_rgb(60, 150, 225),
        Hardware::Zippity => C::from_rgb(245, 190, 40),
        Hardware::RockItTwist => C::from_rgb(235, 80, 120),
        Hardware::LeapPad => C::from_rgb(90, 140, 230),
        Hardware::Didj => C::from_rgb(45, 95, 200),
        Hardware::Explorer => C::from_rgb(110, 195, 60),
    }
}

/// A console's short name (its chip).
pub fn name(hw: Hardware) -> &'static str {
    match hw {
        Hardware::Leapster => "Leapster",
        Hardware::LeapsterTv => "Leapster TV",
        Hardware::LeapsterLmax => "L-MAX",
        Hardware::Leapster2 => "Leapster 2",
        Hardware::Zippity => "Zippity",
        Hardware::RockItTwist => "RockIt Twist",
        Hardware::LeapPad => "LeapPad",
        Hardware::Didj => "Didj",
        Hardware::Explorer => "Explorer",
    }
}

fn mix(a: C, b: C, t: f32) -> C {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    C::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// A shape's body: its colour, a darker rim, a lighter band along the top
/// (the light from above), as every icon has.
fn body(p: &egui::Painter, r: egui::Rect, round: f32, base: C) {
    p.rect_filled(r.translate(egui::vec2(0.0, r.height() * 0.06)), round, C::from_black_alpha(70));
    p.rect_filled(r, round, mix(base, C::BLACK, 0.25));
    let inner = r.shrink(r.height() * 0.04);
    p.rect_filled(inner, round * 0.9, base);
    let top = egui::Rect::from_min_max(inner.min, egui::pos2(inner.max.x, inner.center().y));
    p.rect_filled(top.shrink2(egui::vec2(inner.width() * 0.04, 0.0)), round * 0.8, mix(base, C::WHITE, 0.22).gamma_multiply(0.85));
    p.rect_filled(egui::Rect::from_min_max(egui::pos2(inner.min.x, inner.center().y - 1.0), inner.max).shrink2(egui::vec2(inner.width() * 0.02, 0.0)), round * 0.8, base);
}

/// A dark glass screen with a glint.
fn screen(p: &egui::Painter, r: egui::Rect) {
    p.rect_filled(r.expand(r.height() * 0.08), r.height() * 0.1, C::from_gray(30));
    p.rect_filled(r, r.height() * 0.06, C::from_rgb(28, 52, 64));
    let glint = vec![r.min + egui::vec2(r.width() * 0.08, r.height() * 0.1), r.min + egui::vec2(r.width() * 0.4, r.height() * 0.1), r.min + egui::vec2(r.width() * 0.15, r.height() * 0.75)];
    p.add(egui::Shape::convex_polygon(glint, C::from_white_alpha(28), egui::Stroke::NONE));
}

fn dpad(p: &egui::Painter, c: egui::Pos2, s: f32) {
    let col = C::from_gray(50);
    p.rect_filled(egui::Rect::from_center_size(c, egui::vec2(s, s * 0.34)), s * 0.08, col);
    p.rect_filled(egui::Rect::from_center_size(c, egui::vec2(s * 0.34, s)), s * 0.08, col);
}

fn button(p: &egui::Painter, c: egui::Pos2, r: f32, col: C) {
    p.circle_filled(c + egui::vec2(0.0, r * 0.15), r, mix(col, C::BLACK, 0.4));
    p.circle_filled(c, r, col);
    p.circle_filled(c - egui::vec2(r * 0.3, r * 0.3), r * 0.35, C::from_white_alpha(70));
}

/// The tile behind the device: a soft panel that reads on dark and light
/// backgrounds alike.
fn tile(p: &egui::Painter, rect: egui::Rect, radius: f32, hw: Hardware) {
    let tint = color(hw);
    p.rect_filled(rect, radius, mix(C::from_rgb(34, 38, 44), tint, 0.18));
    let glow = egui::Rect::from_center_size(rect.center() - egui::vec2(0.0, rect.height() * 0.08), rect.size() * 0.8);
    p.rect_filled(glow, glow.width() * 0.5, mix(C::from_rgb(40, 44, 50), tint, 0.28).gamma_multiply(0.6));
}

/// Draws `hw`'s icon filling `rect` (square best), on its tile.
pub fn paint(p: &egui::Painter, rect: egui::Rect, radius: f32, hw: Hardware) {
    tile(p, rect, radius, hw);
    paint_device(p, rect, hw);
}

/// The device alone (no tile), for chips.
pub fn paint_device(p: &egui::Painter, rect: egui::Rect, hw: Hardware) {
    let u = rect.width().min(rect.height()) / 100.0;
    let c = rect.center();
    let v = |x: f32, y: f32| egui::vec2(x * u, y * u);
    let at = |x: f32, y: f32| c + v(x, y);
    let stroke = |w: f32, col: C| egui::Stroke::new(w * u, col);
    match hw {
        // The Leapster family: a wide handheld whose grips hang below it,
        // the screen in the middle, d-pad left, A/B right, and the stylus
        // tucked along the top right. Silver (Leapster), blue (Leapster 2).
        Hardware::Leapster | Hardware::Leapster2 => {
            let base = if hw == Hardware::Leapster { C::from_rgb(200, 206, 214) } else { C::from_rgb(60, 150, 225) };
            p.line_segment([at(14.0, -25.0), at(40.0, -25.0)], stroke(3.5, C::from_rgb(250, 190, 40)));
            p.circle_filled(at(40.0, -25.0), 2.2 * u, C::from_gray(80));
            for x in [-30.0, 30.0] {
                p.circle_filled(at(x, 14.0), 17.0 * u, mix(base, C::BLACK, 0.25));
                p.circle_filled(at(x, 13.0), 15.5 * u, base);
            }
            body(p, egui::Rect::from_center_size(at(0.0, -2.0), v(90.0, 40.0)), 14.0 * u, base);
            screen(p, egui::Rect::from_center_size(at(0.0, -2.0), v(32.0, 26.0)));
            dpad(p, at(-31.0, 2.0), 15.0 * u);
            button(p, at(26.0, 7.0), 5.0 * u, C::from_rgb(230, 70, 60));
            button(p, at(35.0, -2.0), 5.0 * u, C::from_rgb(250, 190, 40));
        }
        // L-MAX: the Leapster shape in orange in front of the TV it drives.
        Hardware::LeapsterLmax => {
            let tv = egui::Rect::from_center_size(at(0.0, -18.0), v(70.0, 46.0));
            p.rect_filled(tv, 5.0 * u, C::from_gray(55));
            p.rect_filled(tv.shrink(4.0 * u), 2.0 * u, C::from_rgb(28, 52, 64));
            p.line_segment([at(-12.0, 6.0), at(12.0, 6.0)], stroke(3.0, C::from_gray(55)));
            let base = C::from_rgb(240, 130, 50);
            body(p, egui::Rect::from_center_size(at(0.0, 22.0), v(72.0, 28.0)), 12.0 * u, base);
            screen(p, egui::Rect::from_center_size(at(0.0, 22.0), v(24.0, 17.0)));
            dpad(p, at(-25.0, 23.0), 11.0 * u);
            button(p, at(25.0, 23.0), 4.5 * u, C::from_rgb(60, 120, 230));
        }
        // Leapster TV: a stick controller on a base.
        Hardware::LeapsterTv => {
            body(p, egui::Rect::from_center_size(at(0.0, 12.0), v(78.0, 40.0)), 14.0 * u, C::from_rgb(230, 140, 40));
            p.rect_filled(egui::Rect::from_center_size(at(-14.0, -6.0), v(7.0, 26.0)), 3.0 * u, C::from_gray(60));
            button(p, at(-14.0, -22.0), 9.0 * u, C::from_rgb(60, 120, 230));
            button(p, at(14.0, 12.0), 6.0 * u, C::from_rgb(230, 70, 60));
            button(p, at(28.0, 5.0), 6.0 * u, C::from_rgb(90, 190, 90));
        }
        // Didj: wide and rounded like a capsule, blue grips around a silver
        // face, a small screen in the middle, d-pad left, two buttons
        // right and shoulder bumps on top.
        Hardware::Didj => {
            let blue = C::from_rgb(45, 95, 200);
            for x in [-30.0, 30.0] {
                p.rect_filled(egui::Rect::from_center_size(at(x, -18.0), v(22.0, 10.0)), 5.0 * u, mix(blue, C::BLACK, 0.3));
            }
            body(p, egui::Rect::from_center_size(c, v(96.0, 38.0)), 19.0 * u, blue);
            let face = egui::Rect::from_center_size(c, v(50.0, 32.0));
            p.rect_filled(face, 10.0 * u, C::from_rgb(205, 212, 222));
            p.rect_filled(egui::Rect::from_min_max(face.min, egui::pos2(face.max.x, face.center().y)).shrink2(v(4.0, 0.0)), 8.0 * u, C::from_rgb(232, 236, 242));
            screen(p, egui::Rect::from_center_size(c, v(28.0, 21.0)));
            dpad(p, at(-36.0, 0.0), 14.0 * u);
            button(p, at(32.0, 5.0), 4.5 * u, C::from_rgb(235, 235, 240));
            button(p, at(40.0, -3.0), 4.5 * u, C::from_rgb(235, 235, 240));
        }
        // Leapster Explorer: a chunky, tall handheld in green, a large
        // touch screen, its stylus standing along the right side, d-pad
        // left and buttons right.
        Hardware::Explorer => {
            let green = C::from_rgb(110, 195, 60);
            p.line_segment([at(44.0, -24.0), at(44.0, 22.0)], stroke(4.0, C::from_rgb(250, 140, 30)));
            p.circle_filled(at(44.0, -25.0), 2.5 * u, C::from_gray(80));
            body(p, egui::Rect::from_center_size(at(-2.0, 0.0), v(80.0, 58.0)), 16.0 * u, green);
            p.rect_filled(egui::Rect::from_center_size(at(-2.0, -1.0), v(46.0, 40.0)), 5.0 * u, C::from_rgb(40, 110, 50));
            screen(p, egui::Rect::from_center_size(at(-2.0, -1.0), v(40.0, 34.0)));
            dpad(p, at(-32.0, 4.0), 13.0 * u);
            button(p, at(27.0, 8.0), 4.5 * u, C::from_rgb(60, 120, 230));
            button(p, at(29.0, -4.0), 4.5 * u, C::from_rgb(250, 200, 50));
        }
        // RockIt Twist: a round handheld whose halves twist (grip ridges on
        // the rim), a small screen at its heart.
        Hardware::RockItTwist => {
            let r = 40.0 * u;
            p.circle_filled(c + v(0.0, 3.0), r, C::from_black_alpha(70));
            p.circle_filled(c, r, C::from_rgb(180, 50, 90));
            p.circle_filled(c, r * 0.92, C::from_rgb(235, 80, 120));
            p.circle_filled(c - v(0.0, 6.0), r * 0.7, C::from_rgb(245, 120, 150).gamma_multiply(0.5));
            for i in 0..12 {
                let a = (i as f32 / 12.0) * std::f32::consts::TAU;
                let d = egui::vec2(a.cos(), a.sin());
                p.line_segment([c + d * r * 0.78, c + d * r * 0.92], stroke(2.5, C::from_rgb(180, 50, 90)));
            }
            screen(p, egui::Rect::from_center_size(c, v(30.0, 24.0)));
        }
        // Zippity: a dome-shaped TV pod with a big ball stick and two big
        // buttons.
        Hardware::Zippity => {
            body(p, egui::Rect::from_center_size(at(0.0, 14.0), v(84.0, 40.0)), 20.0 * u, C::from_rgb(245, 190, 40));
            p.rect_filled(egui::Rect::from_center_size(at(-18.0, -4.0), v(7.0, 22.0)), 3.0 * u, C::from_gray(70));
            button(p, at(-18.0, -20.0), 13.0 * u, C::from_rgb(60, 160, 230));
            button(p, at(12.0, 14.0), 8.0 * u, C::from_rgb(230, 70, 60));
            button(p, at(29.0, 6.0), 8.0 * u, C::from_rgb(90, 190, 90));
        }
        // LeapPad: an open book in a frame, its pen on a cord.
        Hardware::LeapPad => {
            body(p, egui::Rect::from_center_size(at(-6.0, 0.0), v(74.0, 60.0)), 8.0 * u, C::from_rgb(70, 120, 220));
            for x in [-1.0, 1.0] {
                p.rect_filled(egui::Rect::from_center_size(at(-6.0 + x * 14.0, 2.0), v(27.0, 42.0)), 2.0 * u, C::from_rgb(248, 244, 230));
            }
            for y in [-8.0, 0.0, 8.0] {
                p.line_segment([at(-28.0, y), at(-12.0, y)], stroke(1.2, C::from_gray(170)));
            }
            p.line_segment([at(36.0, -22.0), at(42.0, 20.0)], stroke(5.0, C::from_rgb(250, 170, 40)));
            p.circle_filled(at(42.0, 22.0), 2.5 * u, C::from_gray(60));
        }
    }
}

/// The All tab's console filter (a chip clicked), kept in egui's memory.
pub fn filter_id() -> egui::Id {
    egui::Id::new("ribbit-console-filter")
}

/// A small chip: the console's device and name, in its colour.
pub fn chip(p: &egui::Painter, at: egui::Pos2, hw: Hardware) -> egui::Rect {
    let col = color(hw);
    let g = p.layout_no_wrap(name(hw).to_string(), egui::FontId::proportional(11.0), C::WHITE);
    let icon = 18.0;
    let r = egui::Rect::from_min_size(at, egui::vec2(icon + g.size().x + 12.0, g.size().y.max(14.0) + 6.0));
    p.rect_filled(r, 6, mix(C::from_rgb(20, 22, 26), col, 0.22).gamma_multiply(0.94));
    p.rect_stroke(r, 6, egui::Stroke::new(1.0, col), egui::StrokeKind::Inside);
    paint_device(p, egui::Rect::from_center_size(egui::pos2(r.min.x + 3.0 + icon / 2.0, r.center().y), egui::vec2(icon, icon)), hw);
    p.galley(egui::pos2(r.min.x + icon + 7.0, r.center().y - g.size().y / 2.0), g, C::WHITE);
    r
}
