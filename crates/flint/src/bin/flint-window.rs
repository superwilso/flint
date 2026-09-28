//! The window, as its own program.
//!
//! `flint.exe` is a console program, because it is also the command line, and Windows gives every
//! console program a terminal window the moment it starts — double-click it and a black box opens
//! behind Flint. This one is built for the Windows subsystem instead, so it starts with no console
//! at all. It is the same window: everything it does lives in `flint-gui`.
//!
//! There is no terminal to print to, so anything this would print goes in a message box.
#![windows_subsystem = "windows"]

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dark = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] => None,
        ["--dark"] => Some(true),
        ["--light"] => Some(false),
        _ => {
            say("This is the Flint window; it takes only --dark or --light.\n\n\
                 The commands (scan, check, sync, scrobble…) are in the command-line Flint, \
                 flint-cli-windows-x64.exe, run from a terminal.");
            return ExitCode::FAILURE;
        }
    };
    // A panic would otherwise vanish with the process: there is no stderr to land on.
    std::panic::set_hook(Box::new(|info| say(&format!("Flint stopped unexpectedly.\n\n{info}"))));
    match open(dark) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            say(&e);
            ExitCode::FAILURE
        }
    }
}

#[cfg(windows)]
fn open(dark: Option<bool>) -> Result<(), String> {
    flint_gui::win32::run_with(dark)
}

#[cfg(not(windows))]
fn open(_dark: Option<bool>) -> Result<(), String> {
    Err("the window is Windows-only; `flint gui-preview out.svg` draws a picture of it.".into())
}

#[cfg(windows)]
fn say(text: &str) {
    flint_gui::win32::message(text);
}

#[cfg(not(windows))]
fn say(text: &str) {
    eprintln!("flint-window: {text}");
}
