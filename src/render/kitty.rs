//! Minimal kitty graphics protocol driver.
//!
//! The latency killer for image-mode TUIs is re-transmitting pixels on every
//! pan/scroll frame. The kitty protocol splits that in two: transmit the
//! raster once per layout/zoom change (`a=t`), then re-display it with tiny
//! `a=p` commands that crop a source rectangle (pixels) out of the
//! already-uploaded image into a cell rectangle at the cursor. A pan frame
//! is then one CUP plus ~60 bytes.
//!
//! All commands set `q=2` (suppress OK/error responses) so nothing comes
//! back on stdin.

use base64::Engine;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use std::io::Write;

/// Max base64 payload bytes per APC chunk (spec: 4096; all but the last
/// chunk must be a multiple of 4).
const CHUNK: usize = 4096;

fn apc(keys: &str, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + keys.len() + 8);
    out.extend_from_slice(b"\x1b_G");
    out.extend_from_slice(keys.as_bytes());
    if !payload.is_empty() {
        out.push(b';');
        out.extend_from_slice(payload);
    }
    out.extend_from_slice(b"\x1b\\");
    out
}

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::fast());
    enc.write_all(data).expect("in-memory zlib cannot fail");
    enc.finish().expect("in-memory zlib cannot fail")
}

/// Transmit an image without displaying it. RGB (alpha dropped — the raster
/// is opaque), zlib-compressed, base64, chunked. Diagram rasters are flat
/// colors and compress an order of magnitude smaller than raw.
pub fn transmit(id: u32, img: &image::RgbaImage) -> Vec<u8> {
    let mut rgb = Vec::with_capacity((img.width() * img.height() * 3) as usize);
    for p in img.pixels() {
        rgb.extend_from_slice(&p.0[..3]);
    }
    let payload = base64::engine::general_purpose::STANDARD
        .encode(zlib(&rgb))
        .into_bytes();
    let mid = "m=1,q=2";
    let tail = "m=0,q=2";
    // the head carries m itself; a single-chunk stream must terminate with m=0
    let head = format!(
        "a=t,f=24,o=z,i={id},s={},v={},q=2,m={}",
        img.width(),
        img.height(),
        if payload.len() > CHUNK { 1 } else { 0 }
    );

    let mut out = Vec::new();
    let mut chunks = payload.chunks(CHUNK).peekable();
    while let Some(chunk) = chunks.next() {
        let last = chunks.peek().is_none();
        if out.is_empty() {
            out.extend_from_slice(&apc(&head, chunk));
        } else if last {
            out.extend_from_slice(&apc(tail, chunk));
        } else {
            out.extend_from_slice(&apc(mid, chunk));
        }
    }
    out
}

/// Placement parameters for [`put`].
pub struct Put<'a> {
    pub id: u32,
    /// Top-left cell of the destination area.
    pub cell: (u16, u16),
    /// Destination size in cells.
    pub cells: (u16, u16),
    /// Source crop rect in image pixels.
    pub src: &'a (u32, u32, u32, u32),
}

/// Re-display the transmitted image: crop the source rect (image pixels)
/// into the cell area at `cell`. Because the source rect is arbitrary
/// pixels, panning is pixel-exact with no resampling.
pub fn put(p: Put<'_>) -> Vec<u8> {
    let (sx, sy, sw, sh) = p.src;
    let mut out = format!("\x1b[{};{}H", p.cell.1 + 1, p.cell.0 + 1).into_bytes();
    out.extend_from_slice(&apc(
        &format!(
            "a=p,i={},c={},r={},x={sx},y={sy},w={sw},h={sh},q=2",
            p.id, p.cells.0, p.cells.1
        ),
        &[],
    ));
    out
}

/// Delete an image id and free its data (placements vanish; the image is
/// never used again because ids are monotonically allocated).
pub fn delete(id: u32) -> Vec<u8> {
    apc(&format!("a=d,d=I,i={id},q=2"), &[])
}

/// Free every image (shutdown).
pub fn delete_all() -> Vec<u8> {
    apc("a=d,d=A,q=2", &[])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;

    fn tx_payload(cmds: &[u8]) -> Vec<Vec<u8>> {
        // split APC payloads for inspection
        let s = cmds;
        let mut parts = Vec::new();
        let mut i = 0;
        while i + 1 < s.len() {
            if s[i] == 0x1b && s[i + 1] == b'_' {
                let mut end = i + 3;
                while end + 1 < s.len() && !(s[end] == 0x1b && s[end + 1] == b'\\') {
                    end += 1;
                }
                let end = (end + 2).min(s.len());
                parts.push(s[i + 3..end.saturating_sub(2)].to_vec());
                i = end;
            } else {
                i += 1;
            }
        }
        parts
    }

    #[test]
    fn transmit_chunks_and_roundtrips() {
        let img = image::RgbaImage::from_fn(64, 32, |x, y| {
            image::Rgba([(x % 256) as u8, (y % 256) as u8, 0x42, 0xff])
        });
        let cmds = transmit(7, &img);
        let parts = tx_payload(&cmds);
        assert!(!parts.is_empty());
        // every chunk's base64 payload is <= 4096 bytes
        for p in &parts {
            let payload = p.split(|b| *b == b';').nth(1).unwrap();
            assert!(payload.len() <= CHUNK, "{}", payload.len());
        }
        // all but last are multiples of 4
        for p in &parts[..parts.len() - 1] {
            let payload = p.split(|b| *b == b';').nth(1).unwrap();
            assert_eq!(payload.len() % 4, 0);
        }
        // head carries the action and geometry, tail ends the stream
        assert!(parts[0].starts_with(b"a=t,f=24,o=z,i=7,s=64,v=32,q=2,m="));
        assert!(parts.last().unwrap().starts_with(b"m=0,q=2;"));

        // decode: b64 concat -> zlib -> rgb bytes
        let mut b64 = Vec::new();
        for p in &parts {
            b64.extend_from_slice(p.split(|b| *b == b';').nth(1).unwrap());
        }
        let raw = base64::engine::general_purpose::STANDARD
            .decode(&b64)
            .unwrap();
        let mut dec = flate2::read::ZlibDecoder::new(&raw[..]);
        let mut rgb = Vec::new();
        dec.read_to_end(&mut rgb).unwrap();
        assert_eq!(rgb.len(), 64 * 32 * 3);
        assert_eq!(&rgb[0..3], &[0, 0, 0x42]); // pixel (0,0)
        assert_eq!(&rgb[3..6], &[1, 0, 0x42]); // pixel (1,0)
    }

    #[test]
    fn put_is_cup_plus_one_apc() {
        let b = put(Put {
            id: 3,
            cell: (10, 4),
            cells: (50, 30),
            src: &(128, 256, 500, 600),
        });
        let expected = b"\x1b[5;11H\x1b_Ga=p,i=3,c=50,r=30,x=128,y=256,w=500,h=600,q=2\x1b\\";
        assert_eq!(b, expected);
        assert!(b.len() < 100, "pan frames must be tiny, got {}", b.len());
    }

    #[test]
    fn deletes() {
        assert_eq!(delete(9), b"\x1b_Ga=d,d=I,i=9,q=2\x1b\\".to_vec());
        assert_eq!(delete_all(), b"\x1b_Ga=d,d=A,q=2\x1b\\".to_vec());
    }
}
