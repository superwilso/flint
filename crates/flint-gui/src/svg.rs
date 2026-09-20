//! The same paint commands, written out as SVG.
//!
//! **What this is for.** Two things, and it is worth being clear that the second is the important
//! one. It is a preview: a picture of the window that can be made on a machine with no Windows and
//! no display, which is how the window gets designed and reviewed at all here. And it is a
//! regression test with eyes — `flint gui-preview` writes a file, and a change that moves a button
//! shows up in it. Cinder does the same thing with golden pixel hashes for its device UI
//! (`cinder-host --check`); this is the cheaper version of that idea, one file instead of 234.
//!
//! **What it is not.** It is not a renderer. Text is handed to whatever draws the SVG with a font
//! stack that starts at Segoe UI, so a Linux viewer substitutes and the glyphs are not the ones
//! Windows will use. Rectangles, positions and colours are exact; text widths are approximate in
//! exactly the way [`crate::paint::Face::advance`] says.

use crate::paint::{b_of, commands, g_of, r_of, Align, Cmd, Face, Theme};
use crate::Model;

/// The font stacks. Segoe UI is what Windows will actually use; the rest are so the preview reads
/// on a machine that has never had it.
const UI_STACK: &str = "Segoe UI,Selawik,Inter,DejaVu Sans,Helvetica,Arial,sans-serif";
const MONO_STACK: &str = "Consolas,Cascadia Mono,DejaVu Sans Mono,Menlo,monospace";

fn hex(c: u32) -> String {
    format!("#{:02x}{:02x}{:02x}", r_of(c), g_of(c), b_of(c))
}

/// XML's five. Missing one of these turns a path with an `&` in it into a broken file, which is
/// the sort of thing that only ever shows up on someone else's music folder.
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

fn family(f: Face) -> &'static str {
    if f.mono() {
        MONO_STACK
    } else {
        UI_STACK
    }
}

/// Render the command list for `m` at `w`×`h`.
pub fn render(m: &Model, w: i32, h: i32, t: &Theme) -> String {
    draw(&commands(m, w, h, t), w.max(crate::MIN_W), h.max(crate::MIN_H))
}

/// Render an arbitrary command list. Separate from [`render`] so a test can draw one command and
/// check the element it produces.
pub fn draw(cmds: &[Cmd], w: i32, h: i32) -> String {
    let mut s = String::with_capacity(8 * 1024);
    s.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
         viewBox=\"0 0 {w} {h}\" font-kerning=\"normal\">\n"
    ));
    s.push_str(
        "<!-- Written by `flint gui-preview`. Every rectangle and colour here is what the Windows \
         window paints; the glyphs are whatever your viewer substitutes for Segoe UI. -->\n",
    );
    for cmd in cmds {
        match cmd {
            Cmd::Rect { rect, fill, border, radius } => {
                let fill = fill.map(hex).unwrap_or_else(|| "none".into());
                let stroke = match border {
                    // A 1 px stroke straddles the edge, so it is inset by half a pixel; without
                    // this every bordered box is one pixel wider in the preview than in the window.
                    Some(c) => format!(" stroke=\"{}\" stroke-width=\"1\"", hex(*c)),
                    None => String::new(),
                };
                let (x, y, rw, rh) = if border.is_some() {
                    (rect.x as f32 + 0.5, rect.y as f32 + 0.5, (rect.w - 1).max(0), (rect.h - 1).max(0))
                } else {
                    (rect.x as f32, rect.y as f32, rect.w, rect.h)
                };
                let r = if *radius > 0 { format!(" rx=\"{radius}\"") } else { String::new() };
                s.push_str(&format!(
                    "<rect x=\"{x}\" y=\"{y}\" width=\"{rw}\" height=\"{rh}\"{r} fill=\"{fill}\"{stroke}/>\n"
                ));
            }
            Cmd::Text { rect, text, color, face, align } => {
                if text.is_empty() {
                    continue;
                }
                let (x, anchor) = match align {
                    Align::Left => (rect.x, "start"),
                    Align::Center => (rect.x + rect.w / 2, "middle"),
                    Align::Right => (rect.right(), "end"),
                };
                // Centre on the rect: the backends both centre vertically, and `dominant-baseline`
                // is the SVG way of saying so.
                let y = rect.y + rect.h / 2;
                let weight = if face.bold() { " font-weight=\"600\"" } else { "" };
                s.push_str(&format!(
                    "<text x=\"{x}\" y=\"{y}\" font-family=\"{}\" font-size=\"{}\"{weight} \
                     fill=\"{}\" text-anchor=\"{anchor}\" dominant-baseline=\"central\">{}</text>\n",
                    family(*face),
                    face.px(),
                    hex(*color),
                    esc(text)
                ));
            }
            Cmd::Poly { points, color, width } => {
                let pts: Vec<String> = points.iter().map(|(x, y)| format!("{x},{y}")).collect();
                s.push_str(&format!(
                    "<polyline points=\"{}\" fill=\"none\" stroke=\"{}\" stroke-width=\"{width}\" \
                     stroke-linecap=\"round\" stroke-linejoin=\"round\"/>\n",
                    pts.join(" "),
                    hex(*color)
                ));
            }
        }
    }
    s.push_str("</svg>\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Phase, H, W};

    fn shown() -> Model {
        let mut m = Model::new();
        m.library = Some("D:\\Music".into());
        m.volumes[0] = Some("E:\\".into());
        m.status = "ready".into();
        m
    }

    /// The file is well-formed enough to open: one root element, every tag closed, and no stray
    /// angle bracket from an unescaped path.
    #[test]
    fn the_output_is_a_single_well_formed_svg() {
        let mut m = shown();
        m.library = Some("D:\\Music & <More>".into());
        m.log = vec!["a & b".into(), "<not a tag>".into()];
        let out = render(&m, W, H, &Theme::light());
        assert!(out.starts_with("<svg "));
        assert!(out.trim_end().ends_with("</svg>"));
        assert_eq!(out.matches("<svg ").count(), 1);
        assert_eq!(out.matches("</svg>").count(), 1);
        // Everything that opened a <text> closed it.
        assert_eq!(out.matches("<text ").count(), out.matches("</text>").count());
        // The ampersands that survive are entities.
        for (i, _) in out.match_indices('&') {
            let tail = &out[i..];
            assert!(
                ["&amp;", "&lt;", "&gt;", "&quot;", "&apos;"].iter().any(|e| tail.starts_with(e)),
                "a bare & at {i}: {}",
                &tail[..tail.len().min(20)]
            );
        }
        assert!(!out.contains("<not a tag>"));
    }

    /// Every colour the theme puts on screen reaches the file. This is the check that the preview
    /// is of the real window and not of a default-coloured skeleton.
    #[test]
    fn the_palette_reaches_the_file() {
        let t = Theme::light();
        let mut m = shown();
        m.planned = true; // so the primary buttons are live and the accent is used
        let out = render(&m, W, H, &t);
        for (name, c) in [("band", t.band), ("accent", t.accent), ("log", t.log_bg), ("bg", t.bg)] {
            assert!(out.contains(&hex(c)), "{name} ({}) never painted", hex(c));
        }
    }

    /// The preview changes when the model does — otherwise it is not a preview of anything.
    #[test]
    fn the_preview_follows_the_model() {
        let t = Theme::light();
        let idle = render(&shown(), W, H, &t);
        let mut busy = shown();
        busy.phase = Phase::Working;
        busy.progress = Some(0.4);
        busy.say("copying 41/120");
        let busy = render(&busy, W, H, &t);
        assert_ne!(idle, busy);
        assert!(busy.contains("copying 41/120"));
        assert!(busy.contains("Stop"));
        assert!(!idle.contains("Stop"));
    }

    /// A bordered rectangle is inset by half a pixel, so its outline sits on the edge the window
    /// draws it on rather than half a pixel outside.
    #[test]
    fn a_bordered_rect_is_inset_by_half_a_pixel() {
        let out = draw(
            &[Cmd::Rect {
                rect: crate::Rect::new(10, 20, 100, 30),
                fill: Some(0xFFFFFF),
                border: Some(0x000000),
                radius: 4,
            }],
            200,
            100,
        );
        assert!(out.contains("x=\"10.5\" y=\"20.5\" width=\"99\" height=\"29\""), "{out}");
        assert!(out.contains("rx=\"4\""));
    }
}
