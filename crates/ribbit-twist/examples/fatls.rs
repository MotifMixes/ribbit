//! Research: lists a RockIt Twist FAT volume (e.g. the FileSys package file)
//! and copies files out. `fatls <volume> [path-to-extract out-file]`
fn walk(d: &[u8], path: &str, depth: usize) {
    for e in ribbit_twist::fat::list(d, path).unwrap_or_default() {
        if e.name == "." || e.name == ".." {
            continue;
        }
        let p = format!("{path}/{}", e.name);
        println!("{:width$}{} {}", "", p, if e.is_dir { "<dir>".to_string() } else { e.size.to_string() }, width = depth * 2);
        if e.is_dir && depth < 6 {
            walk(d, &p, depth + 1);
        }
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let d = std::fs::read(&a[1]).expect("volume");
    match (a.get(2), a.get(3)) {
        (Some(p), Some(out)) => std::fs::write(out, ribbit_twist::fat::read_file(&d, p).expect("no such file")).unwrap(),
        _ => walk(&d, "", 0),
    }
}
