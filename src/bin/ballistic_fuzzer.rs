//! The bake-scale §13.6 ray fuzzer — a runtime shell, like `bin/bitprobe.rs`.
//!
//! `cargo run --bin ballistic_fuzzer -- --rays 1000000 --seed 7 --out target/fuzz.md`
//!
//! Exit code 1 means the gate did not pass: a violated invariant, a `WalkError`, or a corridor to
//! crew/ammunition that nobody has blessed. The report file is written either way.

fn main() {
    if let Err(error) = overmatch::run_ballistic_fuzzer() {
        eprintln!("ballistic_fuzzer: {error}");
        std::process::exit(1);
    }
}
