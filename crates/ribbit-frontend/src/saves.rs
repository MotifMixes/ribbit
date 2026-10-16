//! Save files (system EEPROM, cartridge save chip, the Leapster 2's
//! internal flash, NAND and SD card): written atomically
//! (temporary file, then rename) so a crash or forced close mid-write can
//! never leave a truncated save, and soon after the game changes them.

use anyhow::{Context, Result};
use ribbit_core::Machine;
use std::path::{Path, PathBuf};

/// Write this long after the last change (the BIOS writes byte by byte).
pub const SETTLE: std::time::Duration = std::time::Duration::from_millis(500);

#[derive(Debug, Clone, Default)]
pub struct SavePaths {
    pub system: Option<PathBuf>,
    pub cart: Option<PathBuf>,
    /// Leapster 2 internal SPI flash (`io::spiflash`), when the machine has one.
    pub flash: Option<PathBuf>,
    /// Leapster 2 SD card (`io::sd`; its written sectors, `io::sparse`).
    pub sd: Option<PathBuf>,
    /// Leapster 2 CIP3 NAND (`io::nand`; its programmed pages).
    pub nand: Option<PathBuf>,
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))
}

impl SavePaths {
    /// Default per-image save files in `dir`: the system EEPROM belongs to
    /// the BIOS, the save chip to the cartridge (named by their CRC32).
    pub fn for_machine(dir: &Path, machine: &Machine) -> Self {
        let bios = machine.firmware().id().crc32;
        Self {
            system: Some(dir.join(format!("system-{bios:08x}.eeprom"))),
            cart: machine.cartridge().map(|c| dir.join(format!("cart-{:08x}.sav", c.id().crc32))),
            flash: machine.io.spi_flash.as_ref().map(|_| dir.join(format!("flash-{bios:08x}.bin"))),
            sd: sd_card(machine).map(|_| dir.join(format!("sdcard-{bios:08x}.img"))),
            nand: machine.io.nand.as_ref().map(|_| dir.join(format!("nand-{bios:08x}.img"))),
        }
    }

    /// Loads existing save files into the machine (missing files are fine).
    pub fn load(&self, machine: &mut Machine) -> Result<()> {
        if let Some(path) = &self.system
            && path.exists()
        {
            let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
            machine.io.eeprom.load(&bytes);
        }
        if let Some(path) = &self.cart
            && path.exists()
            && machine.io.eeprom.cart.is_some()
        {
            let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
            machine.io.eeprom.insert_cart(Some(&bytes));
        }
        if let (Some(path), Some(flash)) = (&self.flash, machine.io.spi_flash.as_mut())
            && path.exists()
        {
            flash.load(&std::fs::read(path).with_context(|| format!("reading {}", path.display()))?);
        }
        if let (Some(path), Some(card)) = (&self.sd, sd_card_mut(machine))
            && path.exists()
            && !card.load_image(&std::fs::read(path).with_context(|| format!("reading {}", path.display()))?)
        {
            anyhow::bail!("{} is not an SD card image", path.display());
        }
        if let (Some(path), Some(nand)) = (&self.nand, machine.io.nand.as_mut())
            && path.exists()
            && !nand.load_image(&std::fs::read(path).with_context(|| format!("reading {}", path.display()))?)
        {
            anyhow::bail!("{} is not a NAND image", path.display());
        }
        Ok(())
    }

    /// Writes whichever saves changed since the last write and clears their
    /// changed flags. Returns how many files were written.
    pub fn flush(&self, machine: &mut Machine) -> Result<usize> {
        let mut written = 0;
        let e = &mut machine.io.eeprom;
        if let Some(path) = &self.system
            && e.dirty
        {
            write_atomic(path, &e.data)?;
            e.dirty = false;
            written += 1;
            tracing::info!(target: "ribbit::saves", path = %path.display(), "system EEPROM saved");
        }
        if let (Some(path), Some(cart)) = (&self.cart, &e.cart)
            && e.cart_dirty
        {
            write_atomic(path, cart)?;
            e.cart_dirty = false;
            written += 1;
            tracing::info!(target: "ribbit::saves", path = %path.display(), "cartridge save written");
        }
        if let (Some(path), Some(flash)) = (&self.flash, machine.io.spi_flash.as_mut())
            && flash.dirty
        {
            write_atomic(path, &flash.data)?;
            flash.dirty = false;
            written += 1;
            tracing::info!(target: "ribbit::saves", path = %path.display(), "internal flash saved");
        }
        if let (Some(path), Some(card)) = (&self.sd, sd_card_mut(machine))
            && card.dirty
        {
            write_atomic(path, &card.image())?;
            card.dirty = false;
            written += 1;
            tracing::info!(target: "ribbit::saves", path = %path.display(), "SD card saved");
        }
        if let (Some(path), Some(nand)) = (&self.nand, machine.io.nand.as_mut())
            && nand.dirty
        {
            write_atomic(path, &nand.image())?;
            nand.dirty = false;
            written += 1;
            tracing::info!(target: "ribbit::saves", path = %path.display(), "NAND saved");
        }
        Ok(written)
    }
}

fn sd_card(machine: &Machine) -> Option<&ribbit_core::io::sd::SdCard> {
    machine.io.sd.as_ref().and_then(|s| s.card.as_ref())
}

fn sd_card_mut(machine: &mut Machine) -> Option<&mut ribbit_core::io::sd::SdCard> {
    machine.io.sd.as_mut().and_then(|s| s.card.as_mut())
}

/// Tracks when the saves last changed, to write them once they settle.
#[derive(Debug, Default)]
pub struct AutoSave {
    changed_at: Option<std::time::Instant>,
}

impl AutoSave {
    /// Call once per frame; writes the saves `SETTLE` after the last change.
    pub fn tick(&mut self, paths: &SavePaths, machine: &mut Machine, now: std::time::Instant) -> Result<()> {
        let e = &machine.io.eeprom;
        let flash = machine.io.spi_flash.as_ref().is_some_and(|f| f.dirty)
            || sd_card(machine).is_some_and(|c| c.dirty)
            || machine.io.nand.as_ref().is_some_and(|n| n.dirty);
        if !(e.dirty || e.cart_dirty || flash) {
            self.changed_at = None;
            return Ok(());
        }
        let since = *self.changed_at.get_or_insert(now);
        if now.duration_since(since) >= SETTLE {
            paths.flush(machine)?;
            self.changed_at = None;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine_with_cart() -> Machine {
        let fw = ribbit_memory::Firmware::from_bytes(vec![0u8; 0x100]).unwrap();
        let mut rom = vec![0u8; 0x1000];
        rom[..4].copy_from_slice(b"LEAP");
        let cart = ribbit_memory::Cartridge::from_bytes(rom).ok();
        Machine::new(fw, cart)
    }

    #[test]
    fn flush_writes_only_changed_saves_atomically() {
        let dir = std::env::temp_dir().join(format!("ribbit-saves-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let paths = SavePaths { system: Some(dir.join("sys.eeprom")), cart: Some(dir.join("game.sav")), ..Default::default() };
        let mut m = machine_with_cart();
        assert_eq!(paths.flush(&mut m).unwrap(), 0, "nothing changed");
        m.io.eeprom.data[3] = 0x42;
        m.io.eeprom.dirty = true;
        assert_eq!(paths.flush(&mut m).unwrap(), 1);
        assert_eq!(std::fs::read(dir.join("sys.eeprom")).unwrap()[3], 0x42);
        assert!(!m.io.eeprom.dirty && !dir.join("sys.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sd_card_is_saved_and_loaded_back() {
        let dir = std::env::temp_dir().join(format!("ribbit-sdsave-{}", std::process::id()));
        let paths = SavePaths { sd: Some(dir.join("sd.img")), ..Default::default() };
        let mut m = machine_with_cart();
        m.io.sd = Some(ribbit_core::io::sd::SdSlot::with_card());
        let card = sd_card_mut(&mut m).unwrap();
        card.sectors.insert(1000, vec![0x5A; 512]);
        card.dirty = true;
        assert_eq!(paths.flush(&mut m).unwrap(), 1);
        let mut fresh = machine_with_cart();
        fresh.io.sd = Some(ribbit_core::io::sd::SdSlot::with_card());
        paths.load(&mut fresh).unwrap();
        assert_eq!(sd_card(&fresh).unwrap().sectors, sd_card(&m).unwrap().sectors);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn autosave_waits_for_changes_to_settle() {
        let dir = std::env::temp_dir().join(format!("ribbit-autosave-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let paths = SavePaths { system: Some(dir.join("sys.eeprom")), ..Default::default() };
        let mut m = machine_with_cart();
        let mut auto = AutoSave::default();
        let t0 = std::time::Instant::now();
        m.io.eeprom.dirty = true;
        auto.tick(&paths, &mut m, t0).unwrap();
        assert!(m.io.eeprom.dirty, "not yet");
        auto.tick(&paths, &mut m, t0 + SETTLE).unwrap();
        assert!(!m.io.eeprom.dirty && dir.join("sys.eeprom").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
