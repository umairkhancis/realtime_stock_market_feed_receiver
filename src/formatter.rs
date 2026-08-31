/// Space-separated hex dump, for eyeballing a payload against `tcpdump -X`.
pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            s.push(if i % 16 == 0 { '\n' } else { ' ' });
        }
        s.push_str(&format!("{b:02X}"));
    }
    s
}

/// Renders an ITCH price (scaled by 10,000) as a decimal string.
///
/// Integer math only — there are no floats in ITCH and there should be none in
/// the codec. Formatting belongs at the display layer, which is here.
pub fn format_price(scaled: u32) -> String {
    format!("{}.{:04}", scaled / 10_000, scaled % 10_000)
}

use figlet_rs::FIGfont;
use colored::*;

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
