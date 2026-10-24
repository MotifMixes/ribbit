//! Prints the touch layout a cartridge holds (src/layout.rs):
//! `lplayout <cartridge> [all]` (all: every clickable's outline too), or
//! `lplayout <cartridge> book <out.toml>` to write it as a book file, or
//! `lplayout <cartridge> regions` to print the drawn book's spots (spread,
//! region, name, page 0/1, x0, y0, x1, y1; tab-separated) for
//! scripts/leappad_words.py.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let cart = std::fs::read(&a[1]).expect("cartridge");
    if a.get(2).is_some_and(|x| x == "book") {
        let title = std::path::Path::new(&a[1]).file_stem().map_or("LeapPad book".into(), |s| s.to_string_lossy().into_owned());
        match ribbit_leappad::book::Book::from_cartridge(&cart, &title, None) {
            Ok(b) => std::fs::write(&a[3], b.to_toml()).expect("write the book"),
            Err(e) => println!("no layout: {e}"),
        }
        return;
    }
    if a.get(2).is_some_and(|x| x == "regions") {
        let b = ribbit_leappad::book::Book::from_cartridge(&cart, "regions", None).expect("the cartridge's layout");
        for (i, s) in b.spreads.iter().enumerate() {
            for (k, r) in s.regions.iter().enumerate() {
                let [x0, y0, x1, y1] = r.rect;
                let page = (r.page == ribbit_leappad::book::Side::Right) as u8;
                println!("{i}	{k}	{}	{page}	{x0}	{y0}	{x1}	{y1}", r.name);
            }
        }
        return;
    }
    let all = a.get(2).is_some_and(|x| x == "all");
    let l = match ribbit_leappad::layout::read(&cart) {
        Ok(l) => l,
        Err(e) => return println!("no layout: {e}"),
    };
    for (i, s) in l.spreads.iter().enumerate() {
        let v = l.main_view(s);
        let opens = l.views.iter().find(|v| v.index == s.opens).map_or("?", |v| v.name.as_str());
        println!(
            "spread {}: GO {:?} {:?} opens {opens}; touch view {} ({} clickables)",
            i + 1,
            s.go.page,
            s.go.bounds(),
            v.map_or("none", |v| v.name.as_str()),
            v.map_or(0, |v| v.hotspots.len())
        );
        if let (true, Some(v)) = (all, v) {
            for h in &v.hotspots {
                let modes = if h.modes == 0xFF { String::new() } else { format!(" modes {:02X}", h.modes) };
                println!("  {}:{}{modes} {:?} {:?}", h.kind_name(), h.id, h.shape.page, h.shape.outline());
            }
        }
    }
}
