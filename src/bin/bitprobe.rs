fn main() {
    if let Err(error) = overmatch::run_bitprobe() {
        eprintln!("bitprobe: {error}");
        std::process::exit(2);
    }
}
