//! Renders icons/app-icon.svg to the PNG set the desktop bundles need:
//! icons/app-icon-{16,32,48,64,128,256,512}.png plus the 128 RGBA used by
//! the runtime window icon and the .ico for the Windows executable.
//! Run: cargo run --release --example render-icon
use std::path::Path;

fn main() {
    let svg = std::fs::read_to_string("icons/app-icon.svg")
        .expect("read icons/app-icon.svg");
    let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default())
        .expect("parse app-icon.svg");
    let native = tree.size().width();

    for size in [16u32, 32, 48, 64, 128, 256, 512] {
        let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size).expect("pixmap");
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(
                size as f32 / native,
                size as f32 / native,
            ),
            &mut pixmap.as_mut(),
        );
        let out = format!("icons/app-icon-{size}.png");
        pixmap.save_png(Path::new(&out)).expect("save png");
        println!("wrote {out}");
    }

    // the Windows .ico packs the square PNG sizes
    let sizes: Vec<(u16, &Path)> = [
        (16u16, Path::new("icons/app-icon-16.png")),
        (32, Path::new("icons/app-icon-32.png")),
        (48, Path::new("icons/app-icon-48.png")),
        (64, Path::new("icons/app-icon-64.png")),
        (128, Path::new("icons/app-icon-128.png")),
        (256, Path::new("icons/app-icon-256.png")),
    ]
    .into_iter()
    .map(|(s, p)| (s, Path::new(p)))
    .collect();
    let mut ico = ico_bytes(&sizes);
    std::fs::write("icons/app-icon.ico", &mut ico).expect("write ico");
    println!("wrote icons/app-icon.ico");
}

/// Minimal ICO container: a 6-byte header, one directory entry per image,
/// then each PNG's bytes (ICO has stored PNG data natively since Vista).
fn ico_bytes(images: &[(u16, &Path)]) -> Vec<u8> {
    let pngs: Vec<(u16, Vec<u8>)> = images
        .iter()
        .map(|(size, path)| (*size, std::fs::read(path).expect("read png")))
        .collect();

    let count = pngs.len() as u16;
    let mut offset = 6 + 16 * pngs.len() as u32;
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // type: icon
    out.extend_from_slice(&count.to_le_bytes());

    for (size, png) in &pngs {
        let side = if *size >= 256 { 0u8 } else { *size as u8 };
        out.push(side); // width
        out.push(side); // height
        out.push(0); // palette
        out.push(0); // reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bpp
        out.extend_from_slice(&(png.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for (_, png) in &pngs {
        out.extend_from_slice(png);
    }
    out
}
