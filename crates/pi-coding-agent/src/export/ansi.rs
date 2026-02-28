/// Convert a string containing ANSI escape codes to HTML with `<span>` styling.
///
/// Handles:
/// - SGR reset (ESC[0m or bare ESC[m)
/// - Bold (1), italic (3), underline (4)
/// - Standard foreground colors 30-37, bright 90-97
/// - Standard background colors 40-47, bright 100-107
/// - 256-color foreground ESC[38;5;Nm and background ESC[48;5;Nm
///
/// All open `<span>` elements are closed on a reset code or at end-of-input.
/// Non-SGR escape sequences are stripped silently.
pub fn ansi_to_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 2);
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    // Current render state
    let mut fg: Option<&str> = None;    // CSS color string (static or heap-allocated — we use &str from table or owned via String)
    let mut bg: Option<&str> = None;
    let mut bold = false;
    let mut italic = false;
    let mut underline = false;
    let mut span_open = false;

    // Because we need owned strings for 256-color computed values, keep them alive here.
    let mut fg_owned: Option<String> = None;
    let mut bg_owned: Option<String> = None;

    /// Close an open span if one exists.
    fn close_span(out: &mut String, span_open: &mut bool) {
        if *span_open {
            out.push_str("</span>");
            *span_open = false;
        }
    }

    /// Emit a new `<span style="...">` based on current state.
    fn open_span(
        out: &mut String,
        span_open: &mut bool,
        fg: Option<&str>,
        bg: Option<&str>,
        bold: bool,
        italic: bool,
        underline: bool,
    ) {
        // Only open a span if there is at least one style to apply.
        if fg.is_none() && bg.is_none() && !bold && !italic && !underline {
            return;
        }
        let mut style = String::new();
        if let Some(color) = fg {
            style.push_str("color:");
            style.push_str(color);
            style.push(';');
        }
        if let Some(color) = bg {
            style.push_str("background-color:");
            style.push_str(color);
            style.push(';');
        }
        if bold {
            style.push_str("font-weight:bold;");
        }
        if italic {
            style.push_str("font-style:italic;");
        }
        if underline {
            style.push_str("text-decoration:underline;");
        }
        out.push_str("<span style=\"");
        out.push_str(&style);
        out.push_str("\">");
        *span_open = true;
    }

    while i < len {
        // Detect ESC character (0x1B).
        if bytes[i] == 0x1B {
            // Look for '[' (CSI introducer).
            if i + 1 < len && bytes[i + 1] == b'[' {
                // Collect the parameter bytes until a final byte (0x40–0x7E).
                let param_start = i + 2;
                let mut j = param_start;
                while j < len && bytes[j] >= 0x20 && bytes[j] < 0x40 {
                    j += 1;
                }
                // j now points at the final byte (or end of input).
                if j < len {
                    let final_byte = bytes[j];
                    if final_byte == b'm' {
                        // SGR sequence — parse parameters.
                        let params_str =
                            std::str::from_utf8(&bytes[param_start..j]).unwrap_or("");

                        // Close existing span before potentially emitting a new one.
                        close_span(&mut out, &mut span_open);

                        apply_sgr_params(
                            params_str,
                            &mut fg,
                            &mut bg,
                            &mut bold,
                            &mut italic,
                            &mut underline,
                            &mut fg_owned,
                            &mut bg_owned,
                        );

                        // Re-derive references from owned strings after the update.
                        let fg_ref: Option<&str> = fg_owned.as_deref().or(fg);
                        let bg_ref: Option<&str> = bg_owned.as_deref().or(bg);

                        open_span(
                            &mut out,
                            &mut span_open,
                            fg_ref,
                            bg_ref,
                            bold,
                            italic,
                            underline,
                        );
                    }
                    // Skip over the entire escape sequence regardless of final byte.
                    i = j + 1;
                } else {
                    // Unterminated escape sequence — skip ESC.
                    i += 1;
                }
            } else {
                // ESC not followed by '[' — skip ESC byte.
                i += 1;
            }
        } else {
            // Regular character — HTML-escape and emit.
            // Handle multi-byte UTF-8 correctly by decoding the full character.
            match bytes[i] {
                b'&' => { out.push_str("&amp;"); i += 1; }
                b'<' => { out.push_str("&lt;"); i += 1; }
                b'>' => { out.push_str("&gt;"); i += 1; }
                b'"' => { out.push_str("&quot;"); i += 1; }
                b if b < 0x80 => { out.push(b as char); i += 1; }
                _ => {
                    // Multi-byte UTF-8: decode from the byte slice
                    let remaining = &input[i..];
                    if let Some(ch) = remaining.chars().next() {
                        out.push(ch);
                        i += ch.len_utf8();
                    } else {
                        i += 1; // skip invalid byte
                    }
                }
            }
        }
    }

    // Close any trailing open span.
    close_span(&mut out, &mut span_open);

    out
}

// ─── SGR parameter application ───────────────────────────────────────────────

/// Apply a semicolon-separated list of SGR parameter codes, mutating the
/// current render state.  Handles 256-color sub-sequences (38;5;N / 48;5;N).
fn apply_sgr_params<'a>(
    params_str: &'a str,
    fg: &mut Option<&'a str>,
    bg: &mut Option<&'a str>,
    bold: &mut bool,
    italic: &mut bool,
    underline: &mut bool,
    fg_owned: &mut Option<String>,
    bg_owned: &mut Option<String>,
) {
    // Split on ';' and collect into a Vec so we can peek ahead for 256-color.
    let codes: Vec<&str> = if params_str.is_empty() {
        vec!["0"]
    } else {
        params_str.split(';').collect()
    };

    let mut idx = 0;
    while idx < codes.len() {
        let code_str = codes[idx].trim();
        let code: u32 = code_str.parse().unwrap_or(0);

        match code {
            0 => {
                // Full reset.
                *fg = None;
                *bg = None;
                *bold = false;
                *italic = false;
                *underline = false;
                *fg_owned = None;
                *bg_owned = None;
            }
            1 => *bold = true,
            3 => *italic = true,
            4 => *underline = true,
            // Standard foreground 30-37.
            30..=37 => {
                *fg_owned = None;
                *fg = Some(standard_color((code - 30) as u8, false));
            }
            // Default foreground.
            39 => {
                *fg = None;
                *fg_owned = None;
            }
            // Standard background 40-47.
            40..=47 => {
                *bg_owned = None;
                *bg = Some(standard_color((code - 40) as u8, false));
            }
            // Default background.
            49 => {
                *bg = None;
                *bg_owned = None;
            }
            // Bright foreground 90-97.
            90..=97 => {
                *fg_owned = None;
                *fg = Some(standard_color((code - 90) as u8, true));
            }
            // Bright background 100-107.
            100..=107 => {
                *bg_owned = None;
                *bg = Some(standard_color((code - 100) as u8, true));
            }
            // 256-color foreground: 38;5;N
            38 => {
                if idx + 2 < codes.len() && codes[idx + 1].trim() == "5" {
                    let n: u8 = codes[idx + 2].trim().parse().unwrap_or(0);
                    *fg_owned = Some(color_256_to_hex(n));
                    *fg = None; // fg_owned takes precedence
                    idx += 2;
                }
            }
            // 256-color background: 48;5;N
            48 => {
                if idx + 2 < codes.len() && codes[idx + 1].trim() == "5" {
                    let n: u8 = codes[idx + 2].trim().parse().unwrap_or(0);
                    *bg_owned = Some(color_256_to_hex(n));
                    *bg = None;
                    idx += 2;
                }
            }
            _ => {}
        }

        idx += 1;
    }
}

// ─── Color tables ─────────────────────────────────────────────────────────────

/// Map a 3-bit color index (0-7) + brightness flag to a CSS hex color string.
const STANDARD_COLORS_NORMAL: [&str; 8] = [
    "#000000", // 0 black
    "#cc0000", // 1 red
    "#4e9a06", // 2 green
    "#c4a000", // 3 yellow
    "#3465a4", // 4 blue
    "#75507b", // 5 magenta
    "#06989a", // 6 cyan
    "#d3d7cf", // 7 white
];

const STANDARD_COLORS_BRIGHT: [&str; 8] = [
    "#555753", // 0 bright black (dark gray)
    "#ef2929", // 1 bright red
    "#8ae234", // 2 bright green
    "#fce94f", // 3 bright yellow
    "#729fcf", // 4 bright blue
    "#ad7fa8", // 5 bright magenta
    "#34e2e2", // 6 bright cyan
    "#eeeeec", // 7 bright white
];

fn standard_color(index: u8, bright: bool) -> &'static str {
    if bright {
        STANDARD_COLORS_BRIGHT[index as usize % 8]
    } else {
        STANDARD_COLORS_NORMAL[index as usize % 8]
    }
}

/// Convert a 256-color index to a CSS `#rrggbb` hex string.
///
/// Layout:
///  0-7   : standard colors (normal palette)
///  8-15  : bright / high-intensity standard colors
///  16-231: 6×6×6 RGB cube
///  232-255: grayscale ramp (dark→light)
fn color_256_to_hex(n: u8) -> String {
    match n {
        0..=7 => STANDARD_COLORS_NORMAL[n as usize].to_string(),
        8..=15 => STANDARD_COLORS_BRIGHT[(n - 8) as usize].to_string(),
        16..=231 => {
            let idx = n - 16;
            let b = idx % 6;
            let g = (idx / 6) % 6;
            let r = idx / 36;
            let channel = |v: u8| if v == 0 { 0u8 } else { 55 + v * 40 };
            format!("#{:02x}{:02x}{:02x}", channel(r), channel(g), channel(b))
        }
        232..=255 => {
            let level = (n - 232) * 10 + 8;
            format!("#{:02x}{:02x}{:02x}", level, level, level)
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Strip surrounding `<span style="...">` and `</span>` tags; just verify
    /// the inner text is present and the span wraps it.
    fn assert_contains_styled(html: &str, text: &str, style_fragment: &str) {
        assert!(
            html.contains(text),
            "Expected text {:?} in {:?}",
            text,
            html
        );
        assert!(
            html.contains(style_fragment),
            "Expected style fragment {:?} in {:?}",
            style_fragment,
            html
        );
    }

    // 1. Plain text passthrough — no ANSI codes should produce no spans.
    #[test]
    fn no_ansi_passthrough() {
        let input = "Hello, world!";
        let result = ansi_to_html(input);
        assert_eq!(result, "Hello, world!");
    }

    // 2. HTML special characters in plain text are escaped.
    #[test]
    fn html_escaping_in_plain_text() {
        let input = "<b>bold</b> & \"quotes\"";
        let result = ansi_to_html(input);
        assert_eq!(result, "&lt;b&gt;bold&lt;/b&gt; &amp; &quot;quotes&quot;");
    }

    // 3. Basic foreground color — red text.
    #[test]
    fn basic_foreground_color_red() {
        // ESC[31m = red foreground, ESC[0m = reset.
        let input = "\x1b[31mred text\x1b[0m";
        let result = ansi_to_html(input);
        assert_contains_styled(&result, "red text", "color:#cc0000");
        assert!(result.ends_with("</span>"), "span must be closed: {result}");
    }

    // 4. Bold + foreground color combination.
    #[test]
    fn bold_and_color_combined() {
        // ESC[1;32m = bold + green foreground.
        let input = "\x1b[1;32mbold green\x1b[0m";
        let result = ansi_to_html(input);
        assert_contains_styled(&result, "bold green", "color:#4e9a06");
        assert_contains_styled(&result, "bold green", "font-weight:bold");
    }

    // 5. Reset closes the span — text after reset must NOT be inside a styled span.
    #[test]
    fn reset_closes_spans() {
        let input = "\x1b[34mblue\x1b[0m plain";
        let result = ansi_to_html(input);
        // After reset, "plain" must appear outside any span.
        let after_close = result.split("</span>").last().unwrap_or("");
        assert!(
            after_close.contains("plain"),
            "Text after reset should be outside span: {result}"
        );
        // Only one span should have been opened.
        assert_eq!(result.matches("<span").count(), 1);
    }

    // 6. Nested / sequential styles — each style segment gets its own span.
    #[test]
    fn sequential_styles_each_get_span() {
        // red then blue, both reset at end.
        let input = "\x1b[31mred\x1b[0m\x1b[34mblue\x1b[0m";
        let result = ansi_to_html(input);
        assert!(result.contains("color:#cc0000"), "red missing: {result}");
        assert!(result.contains("color:#3465a4"), "blue missing: {result}");
        assert_eq!(result.matches("<span").count(), 2);
        assert_eq!(result.matches("</span>").count(), 2);
    }

    // 7. Underline attribute.
    #[test]
    fn underline_attribute() {
        let input = "\x1b[4munderlined\x1b[0m";
        let result = ansi_to_html(input);
        assert_contains_styled(&result, "underlined", "text-decoration:underline");
    }

    // 8. Italic attribute.
    #[test]
    fn italic_attribute() {
        let input = "\x1b[3mitalic text\x1b[0m";
        let result = ansi_to_html(input);
        assert_contains_styled(&result, "italic text", "font-style:italic");
    }

    // 9. Bright / high-intensity foreground colors (90-97).
    #[test]
    fn bright_foreground_colors() {
        // ESC[91m = bright red.
        let input = "\x1b[91mbright red\x1b[0m";
        let result = ansi_to_html(input);
        assert_contains_styled(&result, "bright red", "color:#ef2929");
    }

    // 10. Standard background color (40-47).
    #[test]
    fn standard_background_color() {
        // ESC[42m = green background.
        let input = "\x1b[42mgreen bg\x1b[0m";
        let result = ansi_to_html(input);
        assert_contains_styled(&result, "green bg", "background-color:#4e9a06");
    }

    // 11. Bright background color (100-107).
    #[test]
    fn bright_background_color() {
        // ESC[103m = bright yellow background.
        let input = "\x1b[103mbright yellow bg\x1b[0m";
        let result = ansi_to_html(input);
        assert_contains_styled(&result, "bright yellow bg", "background-color:#fce94f");
    }

    // 12. 256-color foreground (ESC[38;5;Nm).
    #[test]
    fn color_256_foreground() {
        // Index 196 is the 6×6×6 cube entry for pure red: r=5, g=0, b=0 → #ff0000.
        let input = "\x1b[38;5;196mred 256\x1b[0m";
        let result = ansi_to_html(input);
        assert_contains_styled(&result, "red 256", "color:#ff0000");
    }

    // 13. 256-color background (ESC[48;5;Nm).
    #[test]
    fn color_256_background() {
        // Index 21 = r=0, g=0, b=5 → #0000ff
        let input = "\x1b[48;5;21mbg blue 256\x1b[0m";
        let result = ansi_to_html(input);
        assert_contains_styled(&result, "bg blue 256", "background-color:#0000ff");
    }

    // 14. 256-color grayscale ramp.
    #[test]
    fn color_256_grayscale() {
        // Index 232: level = 0*10+8 = 8.
        let result = color_256_to_hex(232);
        assert_eq!(result, "#080808");
        // Index 255: level = 23*10+8 = 238.
        let result = color_256_to_hex(255);
        assert_eq!(result, "#eeeeee");
    }

    // 15. Bare reset ESC[m (no parameter digit) is treated as reset.
    #[test]
    fn bare_reset_no_param() {
        let input = "\x1b[31mred\x1b[mplain";
        let result = ansi_to_html(input);
        let after_close = result.split("</span>").last().unwrap_or("");
        assert!(
            after_close.contains("plain"),
            "Text after bare reset outside span: {result}"
        );
    }

    // 16. No trailing unclosed span at end of input without explicit reset.
    #[test]
    fn implicit_close_at_end_of_input() {
        // No reset at end — parser must still close the span.
        let input = "\x1b[33myellow without reset";
        let result = ansi_to_html(input);
        assert!(result.contains("color:#c4a000"), "color missing: {result}");
        assert!(
            result.ends_with("</span>"),
            "Span must be closed at EOF: {result}"
        );
    }

    // 17. Multiple attributes in one sequence (e.g. bold+italic+underline).
    #[test]
    fn multiple_attributes_in_one_sequence() {
        let input = "\x1b[1;3;4mstyle party\x1b[0m";
        let result = ansi_to_html(input);
        assert_contains_styled(&result, "style party", "font-weight:bold");
        assert_contains_styled(&result, "style party", "font-style:italic");
        assert_contains_styled(&result, "style party", "text-decoration:underline");
    }

    // 18. Non-SGR CSI sequences (e.g. cursor movement ESC[2J) are stripped.
    #[test]
    fn non_sgr_sequences_stripped() {
        // ESC[2J = clear screen (not 'm'), should disappear from output.
        let input = "before\x1b[2Jafter";
        let result = ansi_to_html(input);
        assert_eq!(result, "beforeafter");
    }

    // 19. Empty input produces empty output.
    #[test]
    fn empty_input() {
        assert_eq!(ansi_to_html(""), "");
    }

    // 20. Foreground + background + bold all in one span.
    #[test]
    fn fg_bg_bold_combined() {
        // ESC[1;33;41m = bold + yellow fg + red bg
        let input = "\x1b[1;33;41mhighlight\x1b[0m";
        let result = ansi_to_html(input);
        assert_contains_styled(&result, "highlight", "font-weight:bold");
        assert_contains_styled(&result, "highlight", "color:#c4a000");
        assert_contains_styled(&result, "highlight", "background-color:#cc0000");
    }
}
