//! Clears the user data (players, progress, the play log) from a Zippity
//! NAND dump, as the system's own test mode does (`Zippity::factory_reset`),
//! so the dump starts as a new console would.
//!
//! zipclean <nand.bin> <cleaned.bin>

use ribbit_zippity::Zippity;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, input, output] = &args[..] else {
        eprintln!("usage: zipclean <nand.bin> <cleaned.bin>");
        std::process::exit(2);
    };
    let nand = std::fs::read(input).expect("read the NAND dump");
    let size = nand.len();
    let cleaned = Zippity::factory_reset(nand).unwrap_or_else(|e| {
        eprintln!("the clear did not finish: {e}");
        std::process::exit(1);
    });
    assert_eq!(cleaned.len(), size);
    std::fs::write(output, &cleaned).expect("write the cleaned dump");
    println!("cleared the user data: {output}");
}
