//! Clicky: mechanical keyboard sounds on every key press, as a native macOS
//! menu bar app. See `docs/architecture.md` for the module map.

mod app;
mod audio;
mod bench;
mod diagnostics;
mod engine;
mod input;
mod keys;
mod packs;
mod selftest;
mod settings;
mod ui;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version") => println!("Clicky {}", env!("CARGO_PKG_VERSION")),
        Some("--self-test") => {
            let audible = args.iter().any(|a| a == "--audible");
            if let Err(e) = selftest::run(audible) {
                eprintln!("self-test failed: {e}");
                std::process::exit(1);
            }
        }
        Some("--bench-typing") => {
            if let Err(e) = bench::Plan::parse(&args[1..]).and_then(|plan| bench::run(&plan)) {
                eprintln!("bench failed: {e}");
                std::process::exit(1);
            }
        }
        Some("--menu") => ui::print_menu(args.iter().any(|a| a == "--needs-permission")),
        // Anything else (including LaunchServices' legacy -psn_ argument) starts the app.
        _ => app::run(),
    }
}
