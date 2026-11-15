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
        Hardware::Leapster => C::from_rgb(110, 185, 50),
        Hardware::LeapsterTv => C::from_rgb(240, 195, 40),
        Hardware::LeapsterLmax => C::from_rgb(55, 100, 200),
        Hardware::Leapster2 => C::from_rgb(60, 150, 225),
        Hardware::Zippity => C::from_rgb(245, 140, 40),
        Hardware::RockItTwist => C::from_rgb(150, 90, 215),
        Hardware::LeapPad => C::from_rgb(30, 165, 160),
        Hardware::Didj => C::from_rgb(160, 168, 182),
        Hardware::Explorer => C::from_rgb(160, 205, 40),
        Hardware::LeapPadExplorer => C::from_rgb(95, 185, 60),
        Hardware::LeapPad2 => C::from_rgb(150, 200, 30),
        Hardware::ClickStart => C::from_rgb(70, 185, 110),
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
        Hardware::LeapPadExplorer => "LeapPad Explorer",
        Hardware::LeapPad2 => "LeapPad 2",
        Hardware::ClickStart => "ClickStart",
    }
}

fn mix(a: C, b: C, t: f32) -> C {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    C::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// A dark glass screen with a glint.
fn screen(p: &egui::Painter, r: egui::Rect) {
    p.rect_filled(r.expand(r.height() * 0.08), r.height() * 0.1, C::from_gray(30));
    p.rect_filled(r, r.height() * 0.06, C::from_rgb(28, 52, 64));
    let glint = vec![r.min + egui::vec2(r.width() * 0.08, r.height() * 0.1), r.min + egui::vec2(r.width() * 0.4, r.height() * 0.1), r.min + egui::vec2(r.width() * 0.15, r.height() * 0.75)];
    p.add(egui::Shape::convex_polygon(glint, C::from_white_alpha(28), egui::Stroke::NONE));
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

/// An ellipse turned by `turn` radians, as a polygon (egui's own ellipse
/// does not turn).
fn tilted(p: &egui::Painter, c: egui::Pos2, r: egui::Vec2, turn: f32, col: C) {
    let (s, k) = turn.sin_cos();
    let pts = (0..24)
        .map(|i| {
            let a = i as f32 / 24.0 * std::f32::consts::TAU;
            let (x, y) = (a.cos() * r.x, a.sin() * r.y);
            c + egui::vec2(x * k - y * s, x * s + y * k)
        })
        .collect();
    p.add(egui::Shape::convex_polygon(pts, col, egui::Stroke::NONE));
}

fn oval(p: &egui::Painter, c: egui::Pos2, r: egui::Vec2, col: C) {
    p.add(egui::Shape::ellipse_filled(c, r, col));
}

/// One piece of a case's outline: a rounded rectangle or an ellipse.
#[derive(Clone, Copy)]
enum Part {
    Round(egui::Rect, f32),
    Oval(egui::Pos2, egui::Vec2),
}

/// A case made of overlapping pieces: a drop shadow, a darker rim, its
/// colour and a soft light along the top (the light from above), as every
/// icon has.
fn shell(p: &egui::Painter, parts: &[Part], base: C, u: f32) {
    let fill = |off: egui::Vec2, grow: f32, col: C| {
        for part in parts {
            match *part {
                Part::Round(r, round) => {
                    p.rect_filled(r.translate(off).expand(grow), (round + grow).max(0.0), col);
                }
                Part::Oval(c, r) => oval(p, c + off, r + egui::vec2(grow, grow), col),
            }
        }
    };
    fill(egui::vec2(0.0, 3.0 * u), 0.0, C::from_black_alpha(45));
    fill(egui::Vec2::ZERO, 0.0, mix(base, C::BLACK, 0.3));
    fill(egui::Vec2::ZERO, -1.8 * u, base);
    let b = parts.iter().fold(egui::Rect::NOTHING, |b, part| match *part {
        Part::Round(r, _) => b.union(r),
        Part::Oval(c, r) => b.union(egui::Rect::from_center_size(c, r * 2.0)),
    });
    oval(p, egui::pos2(b.center().x, b.min.y + b.height() * 0.2), egui::vec2(b.width() * 0.36, b.height() * 0.11), C::from_white_alpha(34));
}

/// A grille of short slots (a speaker).
fn grille(p: &egui::Painter, c: egui::Pos2, w: f32, rows: usize, gap: f32, stroke: egui::Stroke) {
    for i in 0..rows {
        let y = c.y + (i as f32 - (rows - 1) as f32 / 2.0) * gap;
        p.line_segment([egui::pos2(c.x - w / 2.0, y), egui::pos2(c.x + w / 2.0, y)], stroke);
    }
}

/// A light plus-shaped D-pad (the Leapster family's, the Didj's).
fn cross(p: &egui::Painter, c: egui::Pos2, s: f32, col: C) {
    let edge = mix(col, C::BLACK, 0.35);
    for (w, h) in [(s, s * 0.38), (s * 0.38, s)] {
        p.rect_filled(egui::Rect::from_center_size(c + egui::vec2(0.0, s * 0.06), egui::vec2(w, h)), s * 0.1, edge);
    }
    for (w, h) in [(s, s * 0.38), (s * 0.38, s)] {
        p.rect_filled(egui::Rect::from_center_size(c, egui::vec2(w, h)), s * 0.1, col);
    }
}

/// The device alone (no tile), for chips.
pub fn paint_device(p: &egui::Painter, rect: egui::Rect, hw: Hardware) {
    let u = rect.width().min(rect.height()) / 100.0;
    let c = rect.center();
    let v = |x: f32, y: f32| egui::vec2(x * u, y * u);
    let at = |x: f32, y: f32| c + v(x, y);
    let stroke = |w: f32, col: C| egui::Stroke::new(w * u, col);
    let rr = |x: f32, y: f32, w: f32, h: f32| egui::Rect::from_center_size(at(x, y), v(w, h));
    match hw {
        // Leapster (the 2005 handheld in its US green, as on its Parent
        // Guide; the 2003 one is blue and silver): green wings either side
        // of a silver face, its lower edge bowed. A big square screen, the
        // D-pad in a raised green ring at the left, A (large) and B on a
        // silver pill at the right over the speaker slots, the red power
        // button top right, and the stylus lying in its tray under the screen.
        Hardware::Leapster => {
            let green = C::from_rgb(120, 196, 40);
            let silver = C::from_rgb(178, 184, 192);
            shell(p, &[Part::Round(rr(0.0, -2.0, 94.0, 54.0), 16.0 * u), Part::Oval(at(0.0, 18.0), v(36.0, 10.0))], green, u);
            p.rect_filled(rr(0.0, 2.0, 56.0, 58.0), 9.0 * u, mix(silver, C::BLACK, 0.3));
            p.rect_filled(rr(0.0, 1.5, 53.0, 55.0), 8.0 * u, silver);
            screen(p, rr(0.0, -6.0, 31.0, 29.0));
            // The stylus in its tray.
            p.rect_filled(rr(0.0, 19.0, 40.0, 7.0), 3.5 * u, mix(silver, C::BLACK, 0.35));
            p.line_segment([at(-17.0, 19.0), at(17.0, 19.0)], stroke(3.6, C::from_gray(225)));
            for x in [-12.0, 12.0] {
                p.rect_filled(rr(x, 19.0, 3.0, 7.0), 1.0 * u, green);
            }
            // D-pad in its green ring, the three small buttons above it.
            p.rect_filled(rr(-36.0, 1.0, 21.0, 21.0), 7.0 * u, mix(green, C::BLACK, 0.3));
            p.rect_filled(rr(-36.0, 0.5, 19.0, 19.0), 6.5 * u, mix(green, C::WHITE, 0.15));
            cross(p, at(-36.0, 0.5), 14.0 * u, mix(green, C::WHITE, 0.35));
            for x in [-43.0, -36.0, -29.0] {
                button(p, at(x, -16.0), 2.2 * u, green);
            }
            // A and B on their silver pill, speaker below, power above.
            p.rect_filled(rr(36.0, -5.0, 22.0, 13.0), 6.5 * u, mix(silver, C::BLACK, 0.3));
            p.rect_filled(rr(36.0, -5.5, 20.5, 11.5), 6.0 * u, silver);
            button(p, at(32.0, -5.5), 4.6 * u, green);
            button(p, at(41.5, -5.5), 3.0 * u, green);
            grille(p, at(36.0, 9.0), 14.0 * u, 4, 2.8 * u, stroke(1.2, mix(green, C::BLACK, 0.45)));
            button(p, at(41.0, -20.0), 2.4 * u, C::from_rgb(225, 40, 35));
        }
        // L-MAX: the silver face held between deep blue grips, a blue
        // D-pad and A button, the stylus on its cord at the top right; the
        // TV it drives behind (its video out is what sets it apart).
        Hardware::LeapsterLmax => {
            let blue = C::from_rgb(40, 90, 175);
            let silver = C::from_rgb(196, 201, 210);
            let tv = rr(-24.0, -27.0, 40.0, 28.0);
            p.rect_filled(tv, 3.0 * u, C::from_gray(60));
            p.rect_filled(tv.shrink(3.0 * u), 1.5 * u, C::from_rgb(70, 130, 200));
            shell(p, &[Part::Round(rr(0.0, 2.0, 84.0, 44.0), 13.0 * u), Part::Oval(at(-33.0, 10.0), v(14.0, 16.0)), Part::Oval(at(33.0, 10.0), v(14.0, 16.0))], blue, u);
            p.rect_filled(rr(0.0, 1.0, 62.0, 42.0), 10.0 * u, silver);
            p.rect_filled(rr(0.0, -11.0, 80.0, 18.0), 9.0 * u, silver);
            oval(p, at(0.0, -15.0), v(30.0, 5.0), C::from_white_alpha(40));
            screen(p, rr(0.0, 0.0, 30.0, 28.0));
            cross(p, at(-27.0, 0.0), 13.0 * u, blue);
            button(p, at(27.0, 1.0), 5.0 * u, blue);
            button(p, at(33.0, -8.0), 3.0 * u, blue);
            // The stylus's cord, and the stylus in its holder at the top right.
            p.add(egui::Shape::line(vec![at(40.0, -16.0), at(46.0, -24.0), at(44.0, -36.0)], stroke(1.2, C::from_rgb(90, 140, 220))));
            p.line_segment([at(30.0, -22.0), at(40.0, -16.0)], stroke(3.0, C::from_gray(225)));
        }
        // Leapster 2 (green and blue): a lime green case with wide round
        // ends, blue grips along their lower edges, a blue D-pad in a green
        // ring, a large blue A with a small B, the round speaker bottom right.
        Hardware::Leapster2 => {
            let lime = C::from_rgb(150, 205, 35);
            let blue = C::from_rgb(35, 85, 185);
            shell(p, &[Part::Round(rr(0.0, 0.0, 64.0, 46.0), 14.0 * u), Part::Oval(at(-30.0, 2.0), v(20.0, 22.0)), Part::Oval(at(30.0, 2.0), v(20.0, 22.0))], blue, u);
            oval(p, at(-28.0, -2.0), v(18.0, 18.0), lime);
            oval(p, at(28.0, -2.0), v(18.0, 18.0), lime);
            p.rect_filled(rr(0.0, -0.5, 60.0, 43.0), 12.0 * u, lime);
            oval(p, at(0.0, -16.0), v(30.0, 6.0), C::from_white_alpha(34));
            p.rect_filled(rr(0.0, -1.0, 34.0, 30.0), 4.0 * u, mix(blue, C::BLACK, 0.2));
            screen(p, rr(0.0, -1.0, 28.0, 25.0));
            oval(p, at(-32.0, 0.0), v(9.5, 9.5), mix(lime, C::BLACK, 0.2));
            cross(p, at(-32.0, 0.0), 13.0 * u, blue);
            button(p, at(30.0, -2.0), 6.0 * u, blue);
            button(p, at(37.0, -11.0), 3.0 * u, blue);
            for r in [3.5f32, 6.0, 8.5] {
                p.add(egui::Shape::circle_stroke(at(31.0, 13.0), r * u, stroke(1.0, mix(lime, C::BLACK, 0.45))));
            }
        }
        // Leapster TV: a round blue console on green feet, a yellow rim
        // round its green top, the yellow joystick standing in the middle,
        // a big red and a small blue button in front, the cartridge slot
        // in its side.
        Hardware::LeapsterTv => {
            let blue = C::from_rgb(45, 75, 190);
            let yellow = C::from_rgb(250, 205, 40);
            let green = C::from_rgb(110, 200, 60);
            for (x, y) in [(-32.0, 30.0), (32.0, 30.0), (0.0, 36.0)] {
                oval(p, at(x, y + 2.0), v(9.0, 5.0), C::from_black_alpha(60));
                oval(p, at(x, y), v(9.0, 5.0), green);
            }
            oval(p, at(0.0, 18.0), v(45.0, 17.0), mix(blue, C::BLACK, 0.25));
            p.rect_filled(egui::Rect::from_min_max(at(-45.0, 8.0), at(45.0, 18.0)), 0.0, mix(blue, C::BLACK, 0.25));
            oval(p, at(0.0, 8.0), v(45.0, 17.0), yellow);
            oval(p, at(0.0, 7.0), v(37.0, 13.0), blue);
            oval(p, at(4.0, 5.0), v(22.0, 8.0), green);
            p.rect_filled(rr(18.0, 22.0, 14.0, 5.0), 1.0 * u, C::from_gray(30));
            button(p, at(-22.0, 9.0), 6.0 * u, C::from_rgb(230, 50, 45));
            button(p, at(-12.0, 15.0), 3.8 * u, C::from_rgb(70, 140, 235));
            p.line_segment([at(4.0, 4.0), at(4.0, -20.0)], stroke(6.0, mix(yellow, C::BLACK, 0.2)));
            oval(p, at(4.0, 4.0), v(6.0, 2.5), mix(yellow, C::BLACK, 0.3));
            button(p, at(4.0, -24.0), 9.0 * u, yellow);
        }
        // Didj: a wide white case round a grey face, a 4:3 screen in the
        // middle, a white D-pad and white A and B on lighter grey panels,
        // the small buttons along its top.
        Hardware::Didj => {
            let white = C::from_rgb(238, 240, 244);
            let grey = C::from_rgb(120, 126, 136);
            shell(p, &[Part::Round(rr(0.0, 0.0, 96.0, 44.0), 16.0 * u)], white, u);
            p.rect_filled(rr(0.0, -1.0, 84.0, 34.0), 9.0 * u, grey);
            for x in [-31.0, 31.0] {
                p.rect_filled(rr(x, 1.0, 20.0, 20.0), 5.0 * u, mix(grey, C::WHITE, 0.25));
            }
            screen(p, rr(0.0, -1.0, 34.0, 26.0));
            cross(p, at(-31.0, 1.0), 13.0 * u, white);
            for (x, y) in [(27.0, 3.0), (35.0, 0.0)] {
                p.rect_filled(rr(x, y + 0.8, 6.5, 6.5), 1.8 * u, mix(white, C::BLACK, 0.35));
                p.rect_filled(rr(x, y, 6.5, 6.5), 1.8 * u, white);
            }
            for x in [-35.0, -27.0, 35.0] {
                p.rect_filled(rr(x, -14.0, 5.0, 3.0), 1.5 * u, white);
            }
        }
        // Leapster Explorer: a lime green case, its top arched, round a
        // charcoal face; a wide screen, the stylus tethered at the top left,
        // the D-pad low on the left, A and B right over a round speaker.
        Hardware::Explorer => {
            let lime = C::from_rgb(140, 200, 40);
            let face = C::from_rgb(52, 56, 62);
            shell(p, &[Part::Round(rr(0.0, 3.0, 94.0, 52.0), 18.0 * u), Part::Oval(at(0.0, -16.0), v(36.0, 12.0))], lime, u);
            p.rect_filled(rr(0.0, 2.0, 78.0, 42.0), 12.0 * u, face);
            oval(p, at(0.0, -12.0), v(28.0, 9.0), face);
            screen(p, rr(0.0, -1.0, 40.0, 28.0));
            cross(p, at(-29.0, 8.0), 12.0 * u, C::from_gray(170));
            button(p, at(28.0, -4.0), 4.0 * u, C::from_gray(185));
            button(p, at(35.0, 3.0), 4.0 * u, C::from_gray(185));
            for (x, y) in [(28.0, -15.0), (35.0, -12.0)] {
                p.circle_filled(at(x, y), 1.8 * u, C::from_gray(150));
            }
            for k in 0..7 {
                let a = k as f32 / 7.0 * std::f32::consts::TAU;
                p.circle_filled(at(31.0 + 3.6 * a.cos(), 14.0 + 3.6 * a.sin()), 0.9 * u, C::from_gray(20));
            }
            p.circle_filled(at(31.0, 14.0), 0.9 * u, C::from_gray(20));
            // The stylus, held at the top left.
            p.line_segment([at(-40.0, -18.0), at(-20.0, -4.0)], stroke(3.5, C::from_gray(200)));
            p.circle_filled(at(-40.0, -18.0), 2.6 * u, lime);
        }
        // LeapPad Explorer (2011): a tablet held upright, a white face in a
        // green shell (the green and white model; its stylus orange), the
        // camera above a tall screen, the big silver D-pad low on the left
        // and the green Home button low on the right; the stylus on its
        // cord at the side.
        Hardware::LeapPadExplorer => {
            let green = C::from_rgb(95, 185, 60);
            let white = C::from_rgb(244, 246, 242);
            shell(p, &[Part::Round(rr(-4.0, 0.0, 52.0, 58.0), 12.0 * u)], green, u);
            p.rect_filled(rr(-4.0, 0.0, 44.0, 50.0), 9.0 * u, white);
            p.circle_filled(at(-4.0, -21.5), 1.6 * u, C::from_gray(40));
            screen(p, rr(-4.0, -5.0, 30.0, 27.0));
            cross(p, at(-14.0, 16.0), 10.0 * u, C::from_gray(175));
            button(p, at(7.0, 16.0), 3.6 * u, C::from_rgb(70, 170, 50));
            // The stylus, on its cord at the right.
            p.line_segment([at(22.0, -10.0), at(30.0, -4.0)], stroke(0.8, C::from_gray(90)));
            p.line_segment([at(30.0, -16.0), at(34.0, 22.0)], stroke(3.0, C::from_rgb(245, 140, 40)));
            p.circle_filled(at(30.0, -16.0), 1.8 * u, C::from_rgb(245, 140, 40));
        }
        // LeapPad 2 (2012), held sideways as its own Connect pictures show
        // it: a lime green shell with rounded ends, a white face, the dark
        // 5-inch screen in the middle, the silver D-pad at the left under
        // the speaker holes, the green Home button at the right, the
        // camera above the screen, the stylus on its cord at the top.
        Hardware::LeapPad2 => {
            let lime = C::from_rgb(150, 200, 30);
            let white = C::from_rgb(244, 246, 242);
            shell(p, &[Part::Round(rr(0.0, 2.0, 76.0, 50.0), 14.0 * u)], lime, u);
            p.rect_filled(rr(0.0, 2.0, 68.0, 42.0), 10.0 * u, white);
            screen(p, rr(0.0, 2.0, 38.0, 28.0));
            p.circle_filled(at(0.0, -15.5), 1.4 * u, C::from_gray(40));
            for k in 0..3 {
                p.circle_filled(at(-27.0 + 3.0 * k as f32, -9.0), 0.9 * u, C::from_gray(110));
            }
            cross(p, at(-26.0, 6.0), 9.0 * u, C::from_gray(175));
            button(p, at(26.0, 6.0), 4.0 * u, C::from_rgb(120, 185, 40));
            // The stylus on its cord, along the top.
            p.line_segment([at(20.0, -21.0), at(30.0, -26.0)], stroke(0.8, C::from_gray(90)));
            p.line_segment([at(-8.0, -26.0), at(30.0, -26.0)], stroke(3.0, lime));
            p.circle_filled(at(-8.0, -26.0), 1.8 * u, lime);
        }
        // ClickStart: a green and white keyboard shaped as a dog, two dark
        // green ears and a face with eyes and a nose above the keys, a red
        // space bar and a blue arrow pad, its paw-shaped mouse pad at the
        // right.
        Hardware::ClickStart => {
            let green = C::from_rgb(120, 200, 70);
            let dark = C::from_rgb(40, 120, 50);
            let white = C::from_rgb(244, 246, 240);
            for x in [-24.0f32, 8.0] {
                tilted(p, at(x, -15.0), v(7.5, 13.0), if x < 0.0 { 0.35 } else { -0.35 }, dark);
            }
            oval(p, at(-8.0, -16.0), v(15.0, 12.0), white);
            p.circle_filled(at(-14.0, -19.0), 2.4 * u, C::from_gray(25));
            p.circle_filled(at(-2.0, -19.0), 2.4 * u, C::from_gray(25));
            oval(p, at(-8.0, -11.5), v(3.6, 2.6), C::from_gray(25));
            shell(p, &[Part::Round(rr(-8.0, 10.0, 74.0, 32.0), 10.0 * u)], green, u);
            p.rect_filled(rr(-13.0, 9.0, 56.0, 22.0), 4.0 * u, white);
            for (row, y) in [3.0f32, 9.0].into_iter().enumerate() {
                for k in 0..8 {
                    let x = -37.0 + 7.0 * k as f32 + 1.5 * row as f32;
                    p.rect_filled(rr(x, y, 5.5, 4.6), 1.0 * u, mix(green, C::WHITE, 0.25));
                }
            }
            p.rect_filled(rr(-13.0, 16.0, 30.0, 4.6), 1.5 * u, C::from_rgb(225, 45, 40));
            cross(p, at(20.0, 9.0), 12.0 * u, C::from_rgb(45, 110, 220));
            // The paw: a pad and three toes.
            oval(p, at(40.0, 14.0), v(8.0, 7.0), mix(green, C::BLACK, 0.3));
            oval(p, at(40.0, 13.5), v(7.0, 6.0), green);
            for (x, y) in [(33.5, 4.0), (40.0, 2.0), (46.5, 4.0)] {
                p.circle_filled(at(x, y), 3.2 * u, mix(green, C::BLACK, 0.3));
                p.circle_filled(at(x, y - 0.4), 2.7 * u, green);
            }
        }
        // RockIt Twist (its purple edition): a square handheld, a black
        // face, the 4:3 screen in the middle and a control on each side: the
        // green 3 x 3 button grid and the Switch along the top, the blue
        // Spinner at the top right, the orange Slider down the right, the
        // three dials along the bottom and the red D-pad at the left.
        Hardware::RockItTwist => {
            let purple = C::from_rgb(135, 75, 200);
            shell(p, &[Part::Round(rr(0.0, 0.0, 86.0, 88.0), 15.0 * u)], purple, u);
            p.rect_filled(rr(0.0, 1.0, 78.0, 80.0), 11.0 * u, C::from_gray(22));
            screen(p, rr(0.0, 3.0, 40.0, 30.0));
            // Grid.
            p.rect_filled(rr(12.0, -29.0, 22.0, 20.0), 3.0 * u, C::from_rgb(60, 200, 70));
            for i in 0..9 {
                let (x, y) = ((i % 3) as f32 * 6.0 + 6.0, (i / 3) as f32 * 5.6 - 34.6);
                p.rect_filled(rr(x, y, 4.2, 3.8), 1.0 * u, C::from_rgb(25, 70, 30));
            }
            // Switch.
            p.rect_filled(rr(-15.0, -31.0, 20.0, 9.0), 4.5 * u, C::from_rgb(170, 110, 235));
            // Spinner.
            p.circle_filled(at(33.0, -22.0), 8.5 * u, C::from_rgb(60, 140, 235));
            p.circle_filled(at(33.0, -22.0), 3.5 * u, C::from_rgb(30, 80, 170));
            // Slider.
            p.rect_filled(rr(34.0, 7.0, 7.0, 26.0), 3.5 * u, C::from_rgb(250, 140, 30));
            // Dials.
            for (x, col) in [(-13.0, C::from_rgb(245, 210, 40)), (0.0, C::from_rgb(70, 200, 70)), (13.0, C::from_rgb(225, 45, 40))] {
                p.rect_filled(rr(x, 32.0, 8.0, 10.0), 2.0 * u, col);
            }
            // D-pad.
            cross(p, at(-31.0, 3.0), 13.0 * u, C::from_rgb(225, 45, 40));
        }
        // Zippity: the round play mat (the orange home spot with its two
        // footprints, the blue oval in front of it, red and green spots to
        // the sides) and the blue Bopper standing beside it, its white
        // handle topped in orange.
        Hardware::Zippity => {
            let blue = C::from_rgb(45, 110, 220);
            oval(p, at(-8.0, 27.0), v(40.0, 17.0), C::from_black_alpha(60));
            oval(p, at(-8.0, 25.0), v(40.0, 17.0), C::from_rgb(235, 240, 245));
            oval(p, at(-8.0, 25.0), v(36.0, 14.5), C::from_rgb(110, 200, 220));
            oval(p, at(-8.0, 17.0), v(11.0, 4.0), blue);
            oval(p, at(-8.0, 27.0), v(11.0, 5.5), C::from_rgb(250, 140, 30));
            for x in [-11.0, -5.0] {
                oval(p, at(x, 27.0), v(1.8, 3.5), C::from_rgb(255, 220, 170));
            }
            oval(p, at(-32.0, 25.0), v(7.0, 4.5), C::from_rgb(230, 50, 45));
            oval(p, at(16.0, 25.0), v(7.0, 4.5), C::from_rgb(90, 195, 60));
            // The Bopper.
            oval(p, at(33.0, 36.0), v(12.0, 5.0), mix(blue, C::BLACK, 0.3));
            oval(p, at(33.0, 35.0), v(11.0, 4.2), blue);
            p.line_segment([at(33.0, 34.0), at(33.0, -14.0)], stroke(6.0, blue));
            p.line_segment([at(31.5, 34.0), at(31.5, -14.0)], stroke(1.5, mix(blue, C::WHITE, 0.35)));
            p.rect_filled(rr(33.0, -24.0, 16.0, 22.0), 7.0 * u, mix(C::WHITE, C::BLACK, 0.2));
            p.rect_filled(rr(33.0, -24.5, 14.5, 20.5), 6.5 * u, C::from_rgb(245, 246, 248));
            p.circle_filled(at(33.0, -27.0), 6.0 * u, C::from_rgb(90, 195, 60));
            button(p, at(33.0, -27.0), 4.3 * u, C::from_rgb(250, 140, 30));
        }
        // LeapPad (1999): a teal case with its carrying handle at the top,
        // the open book on its pale platform, the pen on its cord at the
        // right and the speaker and GO along the bottom.
        Hardware::LeapPad => {
            let teal = C::from_rgb(28, 150, 150);
            p.add(egui::Shape::line(
                (0..=12).map(|i| {
                    let a = std::f32::consts::PI * (1.0 + i as f32 / 12.0);
                    at(-4.0 + 16.0 * a.cos(), -27.0 + 9.0 * a.sin())
                }).collect(),
                stroke(5.0, mix(teal, C::BLACK, 0.2)),
            ));
            shell(p, &[Part::Round(rr(-4.0, 4.0, 80.0, 64.0), 9.0 * u)], teal, u);
            p.rect_filled(rr(-4.0, 1.0, 72.0, 48.0), 4.0 * u, C::from_rgb(196, 230, 226));
            for x in [-1.0, 1.0] {
                p.rect_filled(rr(-4.0 + x * 16.0, 1.0, 31.0, 43.0), 2.0 * u, C::from_rgb(255, 250, 236));
            }
            p.line_segment([at(-4.0, -21.0), at(-4.0, 23.0)], stroke(1.2, C::from_gray(170)));
            for y in [-12.0, -5.0, 2.0, 9.0] {
                p.line_segment([at(-31.0, y), at(-9.0, y)], stroke(1.2, C::from_gray(175)));
            }
            p.rect_filled(rr(10.0, -6.0, 18.0, 14.0), 1.5 * u, C::from_rgb(240, 150, 60));
            button(p, at(28.0, 30.0), 3.2 * u, C::from_rgb(40, 170, 70));
            grille(p, at(-24.0, 30.0), 16.0 * u, 2, 2.6 * u, stroke(1.2, mix(teal, C::BLACK, 0.45)));
            p.add(egui::Shape::line(vec![at(34.0, 30.0), at(42.0, 34.0), at(46.0, 26.0), at(44.0, 18.0)], stroke(1.2, C::from_gray(60))));
            p.line_segment([at(44.0, 18.0), at(38.0, -18.0)], stroke(5.0, C::from_rgb(40, 90, 200)));
            p.line_segment([at(38.0, -18.0), at(37.3, -22.0)], stroke(2.0, C::from_gray(70)));
        }
    }
}

/// The LeapPad models other than the LeapPad itself, in the same style
/// (original art, from the pads' general look: no logos or box art). They
/// are LeapPad books in the library, so they share its colour; the device
/// tells them apart.
///
/// * My First LeapPad: a rounder, chunkier pad for preschoolers, its handle
///   a big arch on top, one landscape page, the pen fat and short on a
///   coiled cord.
/// * Little Touch LeapPad: a soft pad for babies with no pen at all, two
///   thick board pages with finger tabs down the edge and a big round
///   button.
/// * LeapPad Plus Microphone: the LeapPad with the microphone on its stalk
///   at the top right.
pub fn paint_leappad_model(p: &egui::Painter, rect: egui::Rect, model: ribbit_leappad::Model) {
    use ribbit_leappad::Model;
    let u = rect.width().min(rect.height()) / 100.0;
    let c = rect.center();
    let v = |x: f32, y: f32| egui::vec2(x * u, y * u);
    let at = |x: f32, y: f32| c + v(x, y);
    let stroke = |w: f32, col: C| egui::Stroke::new(w * u, col);
    let rr = |x: f32, y: f32, w: f32, h: f32| egui::Rect::from_center_size(at(x, y), v(w, h));
    let paper = C::from_rgb(255, 250, 236);
    match model {
        Model::LeapPad => paint_device(p, rect, Hardware::LeapPad),
        Model::PlusMic => {
            paint_device(p, rect, Hardware::LeapPad);
            p.line_segment([at(30.0, -26.0), at(38.0, -40.0)], stroke(2.5, C::from_gray(70)));
            oval(p, at(39.0, -42.0), v(5.0, 6.0), C::from_gray(50));
            grille(p, at(39.0, -42.0), 5.0 * u, 3, 2.0 * u, stroke(0.8, C::from_gray(120)));
        }
        Model::MyFirst => {
            let blue = C::from_rgb(40, 110, 200);
            let yellow = C::from_rgb(250, 200, 40);
            p.add(egui::Shape::line(
                (0..=14).map(|i| {
                    let a = std::f32::consts::PI * (1.0 + i as f32 / 14.0);
                    at(-6.0 + 22.0 * a.cos(), -26.0 + 14.0 * a.sin())
                }).collect(),
                stroke(8.0, yellow),
            ));
            shell(p, &[Part::Round(rr(-4.0, 6.0, 84.0, 60.0), 16.0 * u)], blue, u);
            p.rect_filled(rr(-6.0, 4.0, 66.0, 42.0), 6.0 * u, mix(blue, C::WHITE, 0.55));
            p.rect_filled(rr(-6.0, 4.0, 58.0, 36.0), 3.0 * u, paper);
            // A picture and a word on the page.
            p.circle_filled(at(-20.0, 0.0), 8.0 * u, C::from_rgb(240, 120, 60));
            for y in [-4.0, 4.0] {
                p.line_segment([at(-6.0, y), at(16.0, y)], stroke(1.6, C::from_gray(170)));
            }
            // GO along the top edge of the page.
            p.circle_filled(at(10.0, -11.0), 3.0 * u, C::from_rgb(40, 170, 70));
            // The fat pen on its coiled cord.
            p.add(egui::Shape::line((0..=16).map(|i| at(34.0 + (i as f32 * 1.4).sin() * 2.0, 30.0 - i as f32 * 2.6)).collect(), stroke(1.2, C::from_gray(60))));
            p.line_segment([at(36.0, -12.0), at(30.0, -30.0)], stroke(8.0, C::from_rgb(220, 50, 50)));
            p.circle_filled(at(30.0, -31.0), 3.5 * u, C::from_rgb(250, 200, 40));
        }
        Model::LittleTouch => {
            let green = C::from_rgb(120, 190, 60);
            let soft = C::from_rgb(245, 245, 235);
            p.add(egui::Shape::line(
                (0..=14).map(|i| {
                    let a = std::f32::consts::PI * (1.0 + i as f32 / 14.0);
                    at(0.0 + 20.0 * a.cos(), -28.0 + 12.0 * a.sin())
                }).collect(),
                stroke(9.0, C::from_rgb(250, 160, 40)),
            ));
            shell(p, &[Part::Round(rr(0.0, 6.0, 90.0, 62.0), 22.0 * u)], green, u);
            p.rect_filled(rr(0.0, 4.0, 74.0, 46.0), 10.0 * u, soft);
            for x in [-1.0, 1.0] {
                p.rect_filled(rr(x * 18.0, 4.0, 33.0, 40.0), 6.0 * u, paper);
            }
            // Thick board pages: finger tabs down the right edge.
            for (i, col) in [C::from_rgb(230, 80, 80), C::from_rgb(70, 140, 230), C::from_rgb(250, 200, 40), C::from_rgb(160, 90, 200)].iter().enumerate() {
                p.rect_filled(rr(37.0, -10.0 + i as f32 * 9.0, 6.0, 7.0), 2.0 * u, *col);
            }
            // A big friendly picture on each page, and the round button.
            p.circle_filled(at(-18.0, 2.0), 9.0 * u, C::from_rgb(250, 170, 60));
            p.circle_filled(at(18.0, 2.0), 9.0 * u, C::from_rgb(90, 170, 230));
            button(p, at(0.0, 31.0), 5.0 * u, C::from_rgb(230, 70, 90));
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
