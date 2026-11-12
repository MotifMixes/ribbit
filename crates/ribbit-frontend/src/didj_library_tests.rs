//! Tests of `didj_library` that reach into the Didj session (a child of
//! `didj`, so they see its private parts).
use crate::didj_library::*;
use std::path::PathBuf;

/// Saves made in each mode open in the others, and go to a real Didj's
/// layout and back: a profile and a game save written as Original,
/// kept by Original+, changed in Enhanced (new), exported, the save
/// wiped, imported. RIBBIT_DIDJ_DIR: a folder of Didj packages.
#[test]
#[ignore]
fn saves_cross_modes_and_a_real_layout() {
    use crate::didj::{DidjSession, Mode};
    let dir = PathBuf::from(std::env::var("RIBBIT_DIDJ_DIR").expect("RIBBIT_DIDJ_DIR"));
    let save_dir = std::env::temp_dir().join(format!("ribbit-didj-modes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&save_dir);
    let profile = ("/Data/0/profile.dsc".to_string(), b"Name=\"ROUND\"\nGrade=\"3\"\nPoints=\"1234\"\n".to_vec());
    let progress = ("/Data/0/POW/GameData.bin".to_string(), vec![7u8; 60]);
    let v = Virtual::open(&dir, &save_dir).unwrap();
    // What Original wrote (as the console writes them to /Didj).
    v.write(&[profile.clone(), progress.clone()]).unwrap();
    let has = |files: &Files, f: &(String, Vec<u8>)| files.iter().any(|g| g == f);
    let mode = |enhanced: bool, hle: bool| Mode { enhanced, cpu_scale: 1, frame_skip: false, hd: 1, hle, smooth: false };
    let serial = std::sync::Arc::new(crate::serial::Serial::default());
    for (name, m) in [("Original", mode(false, false)), ("Original+", mode(true, false))] {
        let s = DidjSession::start(&dir, &save_dir, None, None, None, m, serial.clone()).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(2));
        s.stop();
        let files = v.files().unwrap();
        assert!(has(&files, &profile) && has(&files, &progress), "{name} kept the profile and the save");
    }
    // Enhanced (new): the programs see the files, and a game's new save
    // reaches the NAND.
    let s = DidjSession::start(&dir, &save_dir, None, None, None, mode(true, true), serial.clone()).unwrap();
    let newer = ("/Data/0/POW/GameData.bin".to_string(), vec![9u8; 60]);
    {
        let mut hle = s.shared.hle.lock().unwrap();
        let h = hle.as_mut().expect("Enhanced (new)");
        let files = h.user_files();
        assert!(has(&files, &(profile.0.trim_start_matches('/').to_string(), profile.1.clone())) || has(&files, &profile), "Enhanced (new) sees the profile: {:?}", files.iter().map(|f| &f.0).collect::<Vec<_>>());
        h.p.vfs.add_file(&format!("/Didj{}", newer.0), newer.1.clone());
    }
    s.stop();
    let files = v.files().unwrap();
    assert!(has(&files, &newer), "Enhanced (new)'s save reached the NAND: {:?}", files.iter().map(|f| &f.0).collect::<Vec<_>>());
    // To a real Didj's layout, the save wiped, and back.
    let out = save_dir.join("export");
    let (n, _) = export(&dir, &save_dir, &out).unwrap();
    assert!(n >= 2);
    std::fs::remove_file(crate::didj::save_file(&dir, &save_dir)).unwrap();
    assert!(v.files().unwrap().is_empty());
    assert!(import(&dir, &save_dir, &out).unwrap() >= 2);
    let files = v.files().unwrap();
    assert!(has(&files, &profile) && has(&files, &newer), "imported back");
    // And it starts in Original with them.
    let s = DidjSession::start(&dir, &save_dir, None, None, None, mode(false, false), serial).unwrap();
    s.stop();
    assert!(has(&v.files().unwrap(), &profile));
    let _ = std::fs::remove_dir_all(&save_dir);
}

/// An .lfp installs, lists with its art, survives a start in each mode,
/// and uninstalls with its saves. RIBBIT_DIDJ_DIR as above;
/// RIBBIT_DIDJ_LFP a game's package (or a cartridge dump).
#[test]
#[ignore]
fn install_list_uninstall() {
    let dir = PathBuf::from(std::env::var("RIBBIT_DIDJ_DIR").expect("RIBBIT_DIDJ_DIR"));
    let lfp = PathBuf::from(std::env::var("RIBBIT_DIDJ_LFP").expect("RIBBIT_DIDJ_LFP"));
    let save_dir = std::env::temp_dir().join(format!("ribbit-didj-install-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&save_dir);
    // RIBBIT_DIDJ_SAVE: a set-up console's save to start from (carried
    // over if made on another image); RIBBIT_DIDJ_SAVE_OUT: the save with
    // the game installed is copied there (for didjboot DIDJ_SAVE).
    if let Ok(from) = std::env::var("RIBBIT_DIDJ_SAVE") {
        std::fs::create_dir_all(&save_dir).unwrap();
        std::fs::copy(from, crate::didj::save_file(&dir, &save_dir)).unwrap();
    }
    let (name, folder, art) = install(&dir, &save_dir, &lfp).unwrap();
    if let Ok(to) = std::env::var("RIBBIT_DIDJ_SAVE_OUT") {
        std::fs::copy(crate::didj::save_file(&dir, &save_dir), to).unwrap();
    }
    assert!(art.is_some(), "{name}: menu art");
    let listed = list(&dir, &save_dir).unwrap();
    assert_eq!(listed.iter().filter(|l| installed_folder(&l.game.path) == Some(folder.as_str())).count(), 1);
    assert!(listed.iter().all(|l| !matches!(l.game.art, crate::library::Art::Missing)));
    let serial = std::sync::Arc::new(crate::serial::Serial::default());
    for (enhanced, hle) in [(false, false), (true, false), (true, true)] {
        let m = crate::didj::Mode { enhanced, cpu_scale: 1, frame_skip: false, hd: 1, hle, smooth: false };
        let s = crate::didj::DidjSession::start(&dir, &save_dir, None, None, None, m, serial.clone()).unwrap();
        if hle {
            let h = s.shared.hle.lock().unwrap();
            let p = format!("/Didj/ProgramFiles/{folder}/meta.inf");
            assert!(h.as_ref().unwrap().p.vfs.files_under("/Didj").iter().any(|(f, _)| f.eq_ignore_ascii_case(&p) || f.eq_ignore_ascii_case(p.trim_start_matches("/Didj"))), "Enhanced (new) has the game");
        }
        s.stop();
        assert!(list(&dir, &save_dir).unwrap().iter().any(|l| installed_folder(&l.game.path) == Some(folder.as_str())), "still installed after {enhanced}/{hle}");
    }
    assert!(uninstall(&dir, &save_dir, &folder, true).unwrap() > 0);
    assert!(!list(&dir, &save_dir).unwrap().iter().any(|l| installed_folder(&l.game.path) == Some(folder.as_str())));
    let _ = std::fs::remove_dir_all(&save_dir);
}

/// An Explorer game package installs onto the Explorer's Bulk, lists, is
/// on the NAND the Explorer is built with (`build_explorer`, as the app
/// starts it), and uninstalls. RIBBIT_EXPLORER_DIR: the Explorer's
/// packages; RIBBIT_EXPLORER_LF2: a game package (.lf2 or .lfp), else a
/// stand-in made here (`test_package`, never kept in the repo).
#[test]
#[ignore]
fn explorer_install_list_uninstall() {
    use crate::explorer_library as lx;
    let dir = PathBuf::from(std::env::var("RIBBIT_EXPLORER_DIR").expect("RIBBIT_EXPLORER_DIR"));
    let save_dir = std::env::temp_dir().join(format!("ribbit-lx-install-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&save_dir);
    let pkg = match std::env::var("RIBBIT_EXPLORER_LF2") {
        Ok(p) => PathBuf::from(p),
        Err(_) => {
            let fixtures = std::env::temp_dir().join(format!("ribbit-lx-fixture-{}", std::process::id()));
            std::fs::create_dir_all(&fixtures).unwrap();
            let p = fixtures.join("RibbitTest.lfp");
            std::fs::write(&p, test_package()).unwrap();
            p
        }
    };
    let (name, folder) = lx::install(&dir, &save_dir, &pkg).unwrap();
    let listed = lx::list(&dir, &save_dir).unwrap();
    assert!(listed.iter().any(|l| lx::installed_folder(&l.game.path) == Some(folder.as_str())), "{name} listed");
    // The NAND the app starts the Explorer with has it on Bulk.
    let built = crate::didj::DidjSession::build_explorer(&dir, &save_dir).unwrap();
    let files = ribbit_didj::carry::explorer_files_beyond(&built.lx.bus.nand, &built.original);
    assert!(files.contains_key(&format!("ProgramFiles/{folder}/meta.inf")), "on the Explorer's Bulk");
    drop(built);
    // Export, wipe, import: back again.
    let out = save_dir.join("export");
    assert!(matches!(lx::run(crate::didj_library::Job::Export(out.clone()), &dir, &save_dir), crate::didj_library::Done::Exported { .. }));
    std::fs::remove_file(crate::didj::explorer_save_file(&dir, &save_dir)).unwrap();
    assert!(lx::list(&dir, &save_dir).unwrap().is_empty());
    assert!(matches!(lx::run(crate::didj_library::Job::Import(out), &dir, &save_dir), crate::didj_library::Done::Imported(_)));
    assert_eq!(lx::list(&dir, &save_dir).unwrap().len(), 1, "imported back");
    assert!(matches!(lx::run(crate::didj_library::Job::Uninstall { folder, title: name, delete_saves: true }, &dir, &save_dir), crate::didj_library::Done::Uninstalled { .. }));
    assert!(lx::list(&dir, &save_dir).unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&save_dir);
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("ribbit-lx-fixture-{}", std::process::id())));
}

/// A stand-in Explorer game package, made at test time (never kept in the
/// repo or a games folder): a stored zip in the real layout
/// (RibbitTest/meta.inf, an icon, a stub App.so).
fn test_package() -> Vec<u8> {
    let files: [(&str, &[u8]); 4] = [
        ("RibbitTest/", b""),
        ("RibbitTest/meta.inf", b"Device=\"LeapsterExplorer\"\nType=\"Application\"\nName=\"Ribbit Test Game\"\nVersion=\"1.0.0\"\n"),
        ("RibbitTest/icon.png", b"\x89PNG\r\n\x1a\n"),
        ("RibbitTest/App.so", b"\x7fELF stub"),
    ];
    let (mut out, mut dir) = (Vec::new(), Vec::new());
    for (name, data) in files {
        let crc = !data.iter().fold(!0u32, |c, &b| (0..8).fold(c ^ b as u32, |c, _| if c & 1 != 0 { c >> 1 ^ 0xEDB8_8320 } else { c >> 1 }));
        let local = out.len() as u32;
        let sizes = [crc, data.len() as u32, data.len() as u32];
        out.extend_from_slice(b"PK\x03\x04\x14\x00\x00\x00\x00\x00\x00\x00\x00\x00");
        sizes.iter().for_each(|v| out.extend_from_slice(&v.to_le_bytes()));
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        dir.extend_from_slice(b"PK\x01\x02\x14\x00\x14\x00\x00\x00\x00\x00\x00\x00\x00\x00");
        sizes.iter().for_each(|v| dir.extend_from_slice(&v.to_le_bytes()));
        dir.extend_from_slice(&(name.len() as u16).to_le_bytes());
        dir.extend_from_slice(&[0; 12]);
        dir.extend_from_slice(&local.to_le_bytes());
        dir.extend_from_slice(name.as_bytes());
    }
    let at = out.len() as u32;
    out.extend_from_slice(&dir);
    out.extend_from_slice(b"PK\x05\x06\x00\x00\x00\x00");
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(dir.len() as u32).to_le_bytes());
    out.extend_from_slice(&at.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out
}
