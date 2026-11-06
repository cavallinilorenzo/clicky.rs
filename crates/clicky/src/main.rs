//! Clicky: mechanical keyboard sounds on every key press, as a native macOS
//! menu bar app. See `docs/architecture.md` for the module map.

mod app;
mod audio;
mod diagnostics;
mod engine;
mod input;
mod keys;
mod packs;
mod selftest;
mod settings;
mod ui;

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("--version") => println!("Clicky {}", env!("CARGO_PKG_VERSION")),
        Some("--self-test") => {
            let audible = std::env::args().any(|a| a == "--audible");
            if let Err(e) = selftest::run(audible) {
                eprintln!("self-test failed: {e}");
                std::process::exit(1);
            }
        }
        Some("--menu") => ui::print_menu(),
        // Anything else (including LaunchServices' legacy -psn_ argument) starts the app.
        _ => app::run(),
    }
}
