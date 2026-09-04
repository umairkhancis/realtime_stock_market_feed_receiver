//! The startup splash.
//!
//! The reason this is a module of its own: `figlet-rs` and `colored` are the
//! crate's only third-party dependencies, and this is the only file that names
//! them. Clean Architecture's rule about frameworks is that they belong in the
//! outermost ring where they can be deleted without consequence — and here that
//! is literally true. Removing this file removes both dependencies, and nothing
//! in `domain`, `application` or `infrastructure` notices.
//! `tests/architecture.rs` asserts it.

use colored::*;
use figlet_rs::FIGfont;

pub fn dramatic_display(message: &str) {
    // Load the standard font (includes built-in 3D shading profiles)
    let font = FIGfont::standard().unwrap();

    // Convert your text into an ASCII art layout
    let figure = font.convert(message);

    if let Some(art) = figure {
        // Print the text with a colored shadow effect
        for line in art.to_string().lines() {
            println!("  {}", line.bright_red().bold());
        }
    }
}
