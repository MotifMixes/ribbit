//! Checksums of known dumps, from MAME `leapster.cpp` (ROM_START) and
//! `hash/leapster.xml` (CC0-1.0). Only hashes are stored: no image data.

/// Which hardware an image belongs to. Only `Leapster` is a supported target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hardware {
    /// The original 2003 Leapster (all regional BaseROM revisions).
    Leapster,
    /// Recognised so we can refuse it clearly; not emulated.
    LeapsterTv,
    LeapsterLmax,
    Leapster2,
}

#[derive(Debug, Clone, Copy)]
pub struct KnownImage {
    pub name: &'static str,
    pub hardware: Hardware,
    pub size: usize,
    pub crc32: u32,
    pub sha1: &'static str,
    /// MAME flags this dump as BAD_DUMP.
    pub bad_dump: bool,
}

const fn fw(
    name: &'static str,
    hardware: Hardware,
    size: usize,
    crc32: u32,
    sha1: &'static str,
    bad_dump: bool,
) -> KnownImage {
    KnownImage { name, hardware, size, crc32, sha1, bad_dump }
}

pub const FIRMWARE: &[KnownImage] = &[
    fw(
        "Leapster BaseROM Universal v1.5 (152-10346, 155-10072-a)",
        Hardware::Leapster,
        0x20_0000,
        0xaf05_e5a0,
        "d4468d060543ba7e44785041093bc98bcd9afa07",
        false,
    ),
    fw(
        "Leapster BaseROM US v2.1 (152-11265)",
        Hardware::Leapster,
        0x80_0000,
        0x9639_b3ae,
        "002873b782e823c7a8159deed16c78c149f2afab",
        false,
    ),
    fw(
        "Leapster BaseROM UK v2.1 (152-11452)",
        Hardware::Leapster,
        0x80_0000,
        0xb466_e14d,
        "910c234f03e76b7de55b8aa0a0c62fd1daae4910",
        false,
    ),
    fw(
        "Leapster BaseROM German v2.1 (152-11435)",
        Hardware::Leapster,
        0x80_0000,
        0xa69e_d8ca,
        "e6aacba0c39b1465f344c2b07ff1cbd8a395adac",
        true,
    ),
    fw(
        "Leapster BaseROM Spanish v1.0 (152-11546)",
        Hardware::Leapster,
        0x80_0000,
        0xb433_45e7,
        "31c27e79568115bf36e5ef668f528e3005054152",
        false,
    ),
    fw(
        "Leapster TV Universal v2.1.11 (152-11594)",
        Hardware::LeapsterTv,
        0x20_0000,
        0x194c_c724,
        "000a79d75c19f2e43532ce0b31f0dca0bed49eab",
        false,
    ),
    fw(
        "Leapster 2 CIP3 m9 (152-12659)",
        Hardware::Leapster2,
        0x80_0000,
        0x57bd_e604,
        "4b5eaac1e40bc605eb4cf6d4ad212343334762fd",
        false,
    ),
    fw(
        "Leapster 2 0.6 (152-12206)",
        Hardware::Leapster2,
        0x80_0000,
        0xfa94_d9a7,
        "c5bd84146701dc4a7635b0e37adedb90747adf32",
        false,
    ),
    fw(
        "Leapster Connected B5 (152-12076)",
        Hardware::Leapster2,
        0x80_0000,
        0x4d22_3022,
        "bdc10ad70aa7641716e16fbea16bd0ef35f6e85e",
        false,
    ),
    fw(
        "Leapster L-MAX v2.2 (152-11476)",
        Hardware::LeapsterLmax,
        0x80_0000,
        0xe114_0475,
        "42089165db67005b6a0180e894ff8f36b97a081e",
        false,
    ),
    fw(
        "Leapster L-MAX US v2.1 (152-11238)",
        Hardware::LeapsterLmax,
        0x80_0000,
        0x80bb_4e58,
        "7d8b1c23d08ce76a89cff1112957377c6a1d4b63",
        false,
    ),
];

/// Cartridges used as compatibility targets. Extend as titles are tested.
pub const CARTRIDGES: &[KnownImage] = &[
    fw(
        "SpongeBob SquarePants - Saves the Day (USA) [500-13451-A] (MAME: spongeb)",
        Hardware::Leapster,
        0x80_0000,
        0x5fcf_cf3c,
        "6cc9f4d06ec24678aba1f2c9c856c1f4e25cd612",
        false,
    ),
    fw(
        "SpongeBob SquarePants - Saves the Day (USA, alt) [500-11002 / 500-01168] (MAME: spongeba)",
        Hardware::Leapster,
        0x80_0000,
        0x0f2a_d725,
        "258e5b006086a7fd4cfa4460f3008604a5d87e1d",
        false,
    ),
];

pub fn lookup(table: &'static [KnownImage], id: &crate::ImageId) -> Option<&'static KnownImage> {
    let sha1 = id.sha1_hex();
    table.iter().find(|k| k.size == id.size && k.crc32 == id.crc32 && k.sha1 == sha1)
}
