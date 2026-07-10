fn main() {
    let args: Vec<String> = std::env::args().collect();
    let include_all_time = args.iter().any(|arg| arg == "--all-time");
    if args.iter().any(|arg| arg == "--once") {
        codex_savings_core::run_cli(include_all_time);
        return;
    }

    eprintln!("Usage: codex-savings-cli --once [--all-time]");
    std::process::exit(2);
}
