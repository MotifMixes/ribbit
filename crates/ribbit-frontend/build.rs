//! Builds the User Guide (docs/manual) into the program.

fn main() {
    manual();
}

/// The User Guide (docs/manual): every page (`NN-id.md`, in name order) and
/// picture (`images/*.png`), built into the program as `manual_files.rs` in
/// OUT_DIR.
fn manual() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/manual");
    let list = |dir: &std::path::Path, ext: &str| -> Vec<std::path::PathBuf> {
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext)))
            .collect();
        files.sort();
        files
    };
    println!("cargo:rerun-if-changed={}", root.display());
    println!("cargo:rerun-if-changed={}", root.join("images").display());
    let mut out = String::from("pub static PAGES: &[(&str, &str)] = &[\n");
    for page in list(&root, "md") {
        println!("cargo:rerun-if-changed={}", page.display());
        let stem = page.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
        out.push_str(&format!("    ({stem:?}, include_str!({:?})),\n", page.display().to_string()));
    }
    out.push_str("];\npub static IMAGES: &[(&str, &[u8])] = &[\n");
    for image in list(&root.join("images"), "png") {
        println!("cargo:rerun-if-changed={}", image.display());
        let name = image.file_name().and_then(|s| s.to_str()).unwrap_or_default();
        out.push_str(&format!("    (\"images/{name}\", include_bytes!({:?})),\n", image.display().to_string()));
    }
    out.push_str("];\n");
    let dest = std::path::Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("manual_files.rs");
    std::fs::write(dest, out).expect("writing manual_files.rs");
}
