//! Draws the agentMon app icon in code (tiny-skia, no image files) at every Windows icon size,
//! packs the sizes into one .ico and embeds it in the exe as icon resource 1 (Explorer, Start
//! menu, taskbar; the window loads the same resource). Small sizes get a simpler drawing.
//! Set AGENTMON_ICON_PREVIEW=<folder> to also write each size as a PNG for review.

use std::path::PathBuf;
use tiny_skia::{
    Color, FillRule, GradientStop, LineCap, LineJoin, LinearGradient, Mask, Paint, Path, PathBuilder, Pixmap, Point, Rect,
    SpreadMode, Stroke, Transform,
};

const SIZES: [u32; 10] = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256];

// Palette (design space is 256 x 256).
const INK: (u8, u8, u8) = (0x3a, 0x24, 0x05);
const HONEY_TOP: (u8, u8, u8) = (0xff, 0xda, 0x7a);
const HONEY_BOTTOM: (u8, u8, u8) = (0xe3, 0x8e, 0x0b);
const RIM: (u8, u8, u8) = (0xff, 0xec, 0xb0);
const BODY_TOP: (u8, u8, u8) = (0xff, 0xd8, 0x55);
const BODY_BOTTOM: (u8, u8, u8) = (0xf2, 0xa9, 0x1c);
const WING: (u8, u8, u8) = (0xff, 0xfd, 0xf4);

fn rgba(c: (u8, u8, u8), a: u8) -> Color {
    Color::from_rgba8(c.0, c.1, c.2, a)
}

fn solid(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(c);
    p.anti_alias = true;
    p
}

fn vertical(top: Color, bottom: Color, y0: f32, y1: f32) -> Paint<'static> {
    let mut p = Paint::default();
    p.anti_alias = true;
    p.shader = LinearGradient::new(
        Point::from_xy(0.0, y0),
        Point::from_xy(0.0, y1),
        vec![GradientStop::new(0.0, top), GradientStop::new(1.0, bottom)],
        SpreadMode::Pad,
        Transform::identity(),
    )
    .expect("gradient");
    p
}

fn hexagon(cx: f32, cy: f32, r: f32) -> Path {
    let mut pb = PathBuilder::new();
    for i in 0..6 {
        let a = (-90.0 + 60.0 * i as f32).to_radians();
        let (x, y) = (cx + r * a.cos(), cy + r * a.sin());
        if i == 0 { pb.move_to(x, y) } else { pb.line_to(x, y) }
    }
    pb.close();
    pb.finish().expect("hexagon")
}

fn ellipse(cx: f32, cy: f32, rx: f32, ry: f32) -> Path {
    PathBuilder::from_oval(Rect::from_xywh(cx - rx, cy - ry, rx * 2.0, ry * 2.0).expect("oval")).expect("oval")
}

fn stroke(width: f32) -> Stroke {
    Stroke { width, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() }
}

/// One icon at `size` pixels. `ink` is the outline width in design units, widened for small sizes
/// so the outline never drops under about one pixel.
fn draw(size: u32) -> Pixmap {
    let mut pm = Pixmap::new(size, size).expect("pixmap");
    let s = size as f32 / 256.0;
    let t = Transform::from_scale(s, s);
    let small = size <= 24;
    let ink = (7.0f32).max(1.15 / s);
    let ink_paint = solid(rgba(INK, 255));

    // Honeycomb cell.
    let cell = hexagon(128.0, 128.0, 120.0 - ink * 0.5);
    pm.fill_path(&cell, &vertical(rgba(HONEY_TOP, 255), rgba(HONEY_BOTTOM, 255), 8.0, 248.0), FillRule::Winding, t, None);
    if !small {
        pm.stroke_path(&hexagon(128.0, 128.0, 98.0), &solid(rgba(RIM, 150)), &stroke(4.0), t, None);
        // Shine, top left.
        let shine = ellipse(0.0, 0.0, 46.0, 20.0);
        let st = Transform::from_rotate(-30.0).post_translate(84.0, 62.0).post_scale(s, s);
        pm.fill_path(&shine, &solid(Color::from_rgba8(255, 255, 255, 80)), FillRule::Winding, st, None);
    }
    pm.stroke_path(&cell, &ink_paint, &stroke(ink), t, None);

    // The bee, tilted a little as if it just landed.
    let bee = Transform::from_rotate_at(-10.0, 128.0, 140.0).post_scale(s, s);
    if !small {
        for (cx, cy, rot) in [(104.0, 96.0, -28.0f32), (140.0, 90.0, 18.0)] {
            let w = ellipse(0.0, 0.0, 25.0, 36.0);
            let wt = Transform::from_rotate(rot).post_translate(cx, cy).post_concat(bee);
            pm.fill_path(&w, &solid(rgba(WING, 235)), FillRule::Winding, wt, None);
            pm.stroke_path(&w, &ink_paint, &stroke(ink * 0.75), wt, None);
        }
        // Stinger.
        let mut pb = PathBuilder::new();
        pb.move_to(74.0, 140.0);
        pb.line_to(56.0, 150.0);
        pb.line_to(76.0, 158.0);
        pb.close();
        let sting = pb.finish().expect("stinger");
        pm.fill_path(&sting, &ink_paint, FillRule::Winding, bee, None);
    }
    let (bw, bh) = if small { (62.0, 46.0) } else { (56.0, 41.0) };
    let body = ellipse(124.0, 146.0, bw, bh);
    pm.fill_path(&body, &vertical(rgba(BODY_TOP, 255), rgba(BODY_BOTTOM, 255), 100.0, 190.0), FillRule::Winding, bee, None);
    // Stripes, clipped to the body.
    let mut mask = Mask::new(size, size).expect("mask");
    mask.fill_path(&body, FillRule::Winding, true, bee);
    let stripe_w = if small { 20.0 } else { 15.0 };
    for x in [100.0f32, 134.0] {
        let r = Rect::from_xywh(x - stripe_w / 2.0, 90.0, stripe_w, 120.0).expect("stripe");
        pm.fill_rect(r, &ink_paint, bee, Some(&mask));
    }
    pm.stroke_path(&body, &ink_paint, &stroke(ink), bee, None);
    // Head.
    let (hx, hy, hr) = (184.0, 140.0, if small { 26.0 } else { 24.0 });
    pm.fill_path(&ellipse(hx, hy, hr, hr), &ink_paint, FillRule::Winding, bee, None);
    if !small {
        pm.fill_path(&ellipse(hx + 8.0, hy - 6.0, 6.5, 6.5), &solid(Color::WHITE), FillRule::Winding, bee, None);
        pm.fill_path(&ellipse(hx + 2.0, hy - 11.0, 3.0, 3.0), &solid(Color::WHITE), FillRule::Winding, bee, None);
        // Antennae with round tips.
        for (ex, ey, cx, cy) in [(196.0, 92.0, 182.0, 100.0), (216.0, 104.0, 204.0, 106.0)] {
            let mut pb = PathBuilder::new();
            pb.move_to(hx + (ex - hx) * 0.15, hy - hr + 4.0);
            pb.quad_to(cx, cy, ex, ey);
            let a = pb.finish().expect("antenna");
            pm.stroke_path(&a, &ink_paint, &stroke(ink * 0.7), bee, None);
            pm.fill_path(&ellipse(ex, ey, 6.0, 6.0), &ink_paint, FillRule::Winding, bee, None);
        }
    }
    pm
}

/// An .ico with one PNG image per size (supported by Windows since Vista).
fn ico(pngs: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(pngs.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * pngs.len() as u32;
    for (size, png) in pngs {
        let d = if *size >= 256 { 0u8 } else { *size as u8 };
        out.extend_from_slice(&[d, d, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(png.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for (_, png) in pngs {
        out.extend_from_slice(png);
    }
    out
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=AGENTMON_ICON_PREVIEW");
    println!("cargo:rerun-if-env-changed=AGENTMON_BUILD");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let preview = std::env::var_os("AGENTMON_ICON_PREVIEW").map(PathBuf::from);

    let pngs: Vec<(u32, Vec<u8>)> = SIZES.iter().map(|&n| (n, draw(n).encode_png().expect("png"))).collect();
    if let Some(dir) = &preview {
        let _ = std::fs::create_dir_all(dir);
        for (n, png) in &pngs {
            let _ = std::fs::write(dir.join(format!("icon-{n}.png")), png);
        }
    }
    let ico_path = out.join("agentmon.ico");
    std::fs::write(&ico_path, ico(&pngs)).expect("write ico");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon_with_id(ico_path.to_str().expect("utf-8 path"), "1");
        res.set("ProductName", "agentMon");
        res.set("FileDescription", "agentMon: hive monitor for Claude Code sessions");
        res.compile().expect("embed the icon resource (needs rc.exe from the Windows SDK)");
    }
}
