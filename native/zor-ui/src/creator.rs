//! The buddy creator (Settings > Create a buddy): turns one picture into a pack folder (see pack.rs).
//! The picture is cleaned up (background removed, empty edges cut, mirrored if it faces left), written as `stand.png`
//! next to a `buddy.json`, and the Settings preview shows exactly that file through the bar's own drawing code.

use crate::{cast, gfx::Gfx, pack::{self, Pack}};
use std::path::{Path, PathBuf};
use tiny_skia::Pixmap;

/// Longest side, in px, the picture is worked at. Larger pictures are scaled down when they are read.
pub const WORK: u32 = 480;
/// Pixels with less alpha than this count as empty.
const SEEN: u8 = 8;
/// The largest "remove background" tolerance Settings offers.
pub const MAX_CLEAN: i64 = 80;
/// Longest name a buddy can have (what `pack::parse` keeps).
const NAME_MAX: usize = 40;

/// What the clean-up does: `clean` is the background tolerance (0 leaves the picture alone), `trim` cuts the empty edges,
/// `flip` mirrors the picture.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Opts { pub clean: u8, pub trim: bool, pub flip: bool }

impl Opts {
    /// What identifies a result: it changes when the picture or any option does.
    pub fn key(&self, image: &str) -> String { format!("{image}|{}|{}|{}", self.clean, self.trim, self.flip) }
}

/// The picture at `path`, its longest side at most `WORK` px, aspect kept (None: it can't be read).
pub fn load_source(g: &Gfx, path: &Path) -> Option<Pixmap> {
    let (w, h) = g.image_size(path).filter(|&(w, h)| w > 0 && h > 0)?;
    let k = (WORK as f32 / w.max(h) as f32).min(1.0);
    g.load_image(path, ((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1))
}

/// The colour (blue, green, red) of a premultiplied BGRA pixel before it was premultiplied.
fn straight(p: &[u8]) -> [u8; 3] {
    let a = p[3] as u32;
    if a == 0 || a == 255 { return [p[0], p[1], p[2]]; }
    [0, 1, 2].map(|i| ((p[i] as u32 * 255 + a / 2) / a).min(255) as u8)
}

/// Clears every pixel that joins a corner through pixels within `tol` of that corner's colour (any channel), the way a
/// paint bucket would, so a plain backdrop goes and an island of the same colour inside the character stays. `tol` 0 does
/// nothing. A corner that is already empty is left alone.
pub fn remove_background(px: &mut Pixmap, tol: u8) {
    let (w, h) = (px.width() as usize, px.height() as usize);
    if tol == 0 || w == 0 || h == 0 { return; }
    let data = px.data_mut();
    let mut done = vec![false; w * h];
    for (sx, sy) in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
        let s = sy * w + sx;
        if done[s] || data[s * 4 + 3] < SEEN { continue; }
        let seed = straight(&data[s * 4..s * 4 + 4]);
        let mut stack = vec![s];
        done[s] = true;
        while let Some(i) = stack.pop() {
            data[i * 4..i * 4 + 4].fill(0);
            let (x, y) = (i % w, i / w);
            let around = [(x > 0).then(|| i - 1), (x + 1 < w).then(|| i + 1), (y > 0).then(|| i - w), (y + 1 < h).then(|| i + w)];
            for j in around.into_iter().flatten() {
                if done[j] { continue; }
                let c = &data[j * 4..j * 4 + 4];
                if c[3] < SEEN || straight(c).iter().zip(seed).all(|(a, b)| a.abs_diff(b) <= tol) { done[j] = true; stack.push(j); }
            }
        }
    }
}

/// Smallest rectangle holding every visible pixel, as (left, top, width, height); None for a picture with none.
fn content(px: &Pixmap) -> Option<(u32, u32, u32, u32)> {
    let (w, h) = (px.width(), px.height());
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for y in 0..h {
        for x in 0..w {
            if px.data()[((y * w + x) * 4 + 3) as usize] >= SEEN { (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y)); }
        }
    }
    (x1 >= x0 && y1 >= y0).then(|| (x0, y0, x1 - x0 + 1, y1 - y0 + 1))
}

/// The picture cut to its visible pixels, so its feet are at the bottom edge and it stands in the middle (None: nothing visible).
pub fn trim(px: &Pixmap) -> Option<Pixmap> {
    let (x0, y0, w, h) = content(px)?;
    let mut out = Pixmap::new(w, h)?;
    let src_w = px.width() as usize;
    for row in 0..h as usize {
        let from = ((y0 as usize + row) * src_w + x0 as usize) * 4;
        out.data_mut()[row * w as usize * 4..(row + 1) * w as usize * 4].copy_from_slice(&px.data()[from..from + w as usize * 4]);
    }
    Some(out)
}

/// The picture mirrored left to right.
pub fn flip(px: &Pixmap) -> Pixmap {
    let (w, h) = (px.width() as usize, px.height() as usize);
    let mut out = px.clone();
    for y in 0..h {
        for x in 0..w {
            let (a, b) = ((y * w + x) * 4, (y * w + (w - 1 - x)) * 4);
            out.data_mut()[b..b + 4].copy_from_slice(&px.data()[a..a + 4]);
        }
    }
    out
}

/// `src` with the clean-up applied: background, then empty edges, then the mirror. None when nothing is left to draw.
pub fn process(src: &Pixmap, o: &Opts) -> Option<Pixmap> {
    let mut px = src.clone();
    remove_background(&mut px, o.clean);
    content(&px)?;
    let px = if o.trim { trim(&px)? } else { px };
    Some(if o.flip { flip(&px) } else { px })
}

/// `px` as a compressed PNG file (straight RGBA, best compression, adaptive row filters).
pub fn encode_png(px: &Pixmap) -> Result<Vec<u8>, String> {
    let mut rgba = Vec::with_capacity(px.data().len());
    for p in px.data().chunks(4) { let [b, g, r] = straight(p); rgba.extend([r, g, b, p[3]]); }
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, px.width(), px.height());
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::High);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    w.write_image_data(&rgba).map_err(|e| e.to_string())?;
    w.finish().map_err(|e| e.to_string())?;
    Ok(out)
}

/// A folder name for a buddy called `name`: lowercase letters and digits joined by single dashes, at most 32 characters.
pub fn slug(name: &str) -> String {
    let mut s = String::new();
    for c in name.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() { s.push(c); } else if !s.is_empty() && !s.ends_with('-') { s.push('-'); }
    }
    let s: String = s.chars().take(32).collect();
    let s = s.trim_end_matches('-');
    if s.is_empty() { "buddy".into() } else { s.into() }
}

/// `slug(name)`, or that with -2, -3... when `root` already has a folder of that name.
pub fn unique_id(root: &Path, name: &str) -> String {
    let base = slug(name);
    (1..1000).map(|n| if n == 1 { base.clone() } else { format!("{base}-{n}") }).find(|id| !root.join(id).exists()).unwrap_or(base)
}

/// The text of a `buddy.json` for a buddy with one standing picture. `base` is a built-in character to borrow lines
/// from (anything else means neutral words) and `special` the double-click move (anything unknown is an aura).
pub fn manifest(name: &str, height: u32, base: &str, special: &str) -> String {
    let special = special.trim().to_lowercase();
    let special = if cast::Move::parse(&special).is_some() { special.as_str() } else { "aura" };
    let base = if cast::CASTS.iter().any(|c| c.id == base) { base } else { "" };
    let name: String = name.trim().chars().take(NAME_MAX).collect();
    serde_json::to_string_pretty(&serde_json::json!({
        "name": name, "height": height.clamp(pack::MIN_H as u32, pack::MAX_H as u32), "fps": 8, "base": base,
        "frames": { "stand": ["stand.png"] }, "moves": { "special": special },
    })).unwrap_or_default()
}

/// Makes the pack folder `id` under `root` from a standing picture and a manifest. The manifest must load as a pack and
/// the folder must not exist yet; the folder appears whole or not at all.
pub fn save(root: &Path, id: &str, png: &[u8], manifest: &str) -> Result<(), String> {
    if !pack::safe_id(id) { return Err(format!("\"{id}\" can't be a folder name")); }
    let dir = root.join(id);
    if dir.exists() { return Err(format!("There is already a buddy folder called {id}")); }
    pack::parse(id, &dir, manifest, 0)?;
    let tmp = root.join(format!(".{id}.tmp"));
    let _ = std::fs::remove_dir_all(&tmp);
    let made = std::fs::create_dir_all(&tmp)
        .and_then(|_| std::fs::write(tmp.join("stand.png"), png))
        .and_then(|_| std::fs::write(tmp.join("buddy.json"), manifest))
        .and_then(|_| std::fs::rename(&tmp, &dir));
    if let Err(e) = made { let _ = std::fs::remove_dir_all(&tmp); return Err(e.to_string()); }
    Ok(())
}

/// A picture the creator copied into the images folder (`pick-image` names it "buddy-make-<time>.<ext>"), so it may be deleted
/// once the buddy is saved; nothing else in that folder is ever removed.
pub fn is_own_picture(file: &str) -> bool { file.starts_with("buddy-make-") && pack::safe_file(file) }

/// Where the preview's standing picture is written.
pub fn preview_dir() -> PathBuf { crate::conf::data_dir().join("creator-preview") }

/// The latest clean-up result, for the Settings preview: the picture is on disk in `preview_dir` as a one-picture pack,
/// so the preview draws it through the same loader as a saved buddy. `key` says which picture and options it is for.
pub struct Preview { pub key: String, pub pack: Option<Pack>, pub error: String, pub stamp: u64 }

/// Brings `cur` up to date for `key`: does the work only when the picture or an option changed since last time.
pub fn refresh(cur: &mut Option<Preview>, g: &Gfx, source: &Path, key: &str, o: &Opts, dir: &Path) {
    if cur.as_ref().is_some_and(|p| p.key == key) { return; }
    let stamp = cur.as_ref().map_or(1, |p| p.stamp + 1);
    let made = (|| -> Result<Pack, String> {
        let src = load_source(g, source).ok_or("That picture can't be read.")?;
        let px = process(&src, o).ok_or("Nothing is left of the picture. Lower the background setting.")?;
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        std::fs::write(dir.join("stand.png"), encode_png(&px)?).map_err(|e| e.to_string())?;
        pack::parse("creator", dir, &manifest("Preview", 64, "", "aura"), stamp)
    })();
    *cur = Some(match made {
        Ok(p) => Preview { key: key.into(), pack: Some(p), error: String::new(), stamp },
        Err(error) => Preview { key: key.into(), pack: None, error, stamp },
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: [u8; 4] = [255, 255, 255, 255];
    const RED: [u8; 4] = [0, 0, 255, 255]; // blue, green, red, alpha: premultiplied BGRA
    const NONE: [u8; 4] = [0, 0, 0, 0];

    /// A `w` x `h` picture; `at(x, y)` gives each pixel.
    fn pic(w: u32, h: u32, mut at: impl FnMut(u32, u32) -> [u8; 4]) -> Pixmap {
        let mut p = Pixmap::new(w, h).unwrap();
        for y in 0..h { for x in 0..w { let i = ((y * w + x) * 4) as usize; p.data_mut()[i..i + 4].copy_from_slice(&at(x, y)); } }
        p
    }
    fn at(p: &Pixmap, x: u32, y: u32) -> [u8; 4] { let i = ((y * p.width() + x) * 4) as usize; p.data()[i..i + 4].try_into().unwrap() }
    /// A red 4x4 block at (3, 2) on a white 10x8 picture.
    fn block() -> Pixmap { pic(10, 8, |x, y| if (3..7).contains(&x) && (2..6).contains(&y) { RED } else { WHITE }) }

    #[test]
    fn a_plain_backdrop_goes_and_the_character_stays() {
        let mut p = block();
        remove_background(&mut p, 10);
        assert_eq!((at(&p, 0, 0), at(&p, 9, 7), at(&p, 9, 0), at(&p, 0, 7)), (NONE, NONE, NONE, NONE), "every corner is cleared");
        assert_eq!((at(&p, 3, 2), at(&p, 6, 5), at(&p, 5, 3)), (RED, RED, RED));
        assert_eq!(at(&p, 5, 0), NONE, "the backdrop between the corners goes too, it is all one region");
    }

    #[test]
    fn an_island_of_the_backdrop_colour_inside_the_character_stays() {
        // a white eye (2x2) inside a red body that is closed all round
        let mut p = pic(12, 12, |x, y| if (3..9).contains(&x) && (3..9).contains(&y) { if (5..7).contains(&x) && (5..7).contains(&y) { WHITE } else { RED } } else { WHITE });
        remove_background(&mut p, 10);
        assert_eq!(at(&p, 5, 5), WHITE, "not joined to a corner, so not removed");
        assert_eq!(at(&p, 0, 0), NONE);
    }

    #[test]
    fn the_tolerance_decides_what_counts_as_the_same_colour() {
        let grey = |v: u8| [v, v, v, 255];
        let gradient = |x: u32| grey(250 - x as u8 * 20); // 250, 230, 210, 190
        let mut p = pic(4, 1, |x, _| gradient(x));
        remove_background(&mut p, 0);
        assert_eq!(at(&p, 0, 0), grey(250), "0 does nothing");
        remove_background(&mut p, 20);
        assert_eq!(p.data().chunks(4).filter(|px| px[3] == 0).count(), 4, "20 crosses every step of the gradient from either corner");
        let mut q = pic(4, 1, |x, _| gradient(x));
        remove_background(&mut q, 5);
        assert_eq!((at(&q, 0, 0), at(&q, 3, 0)), (NONE, NONE), "the corners themselves always go");
        assert_eq!((at(&q, 1, 0), at(&q, 2, 0)), (grey(230), grey(210)), "5 is too little to cross a step of 20");
    }

    #[test]
    fn a_transparent_corner_is_not_a_seed_and_soft_edges_compare_by_their_real_colour() {
        let mut p = pic(6, 6, |x, y| if x == 0 && y == 0 { NONE } else { WHITE });
        remove_background(&mut p, 10);
        assert_eq!(at(&p, 5, 5), NONE, "the other corners still seed the fill");
        // a half-transparent white is still white once un-premultiplied
        assert_eq!(straight(&[128, 128, 128, 128]), [255, 255, 255]);
        assert_eq!(straight(&[10, 20, 30, 0]), [10, 20, 30], "a clear pixel is left as it is");
        let all_clear = pic(3, 3, |_, _| NONE);
        let mut q = all_clear.clone();
        remove_background(&mut q, 50);
        assert_eq!(q.data(), all_clear.data());
    }

    #[test]
    fn trimming_keeps_only_the_visible_rectangle() {
        let mut p = block();
        remove_background(&mut p, 10);
        let t = trim(&p).unwrap();
        assert_eq!((t.width(), t.height()), (4, 4));
        assert!(t.data().chunks(4).all(|px| px == RED));
        assert!(trim(&pic(5, 5, |_, _| NONE)).is_none(), "nothing visible");
        let one = pic(7, 5, |x, y| if (x, y) == (6, 4) { RED } else { NONE });
        assert_eq!((trim(&one).unwrap().width(), trim(&one).unwrap().height()), (1, 1), "a lone pixel at the corner");
        assert_eq!(content(&pic(3, 3, |x, _| if x == 1 { [0, 0, 0, SEEN - 1] } else { NONE })), None, "faint specks don't count");
    }

    #[test]
    fn mirroring_swaps_left_and_right() {
        let p = pic(3, 2, |x, y| [x as u8 * 10 + 10, y as u8, 0, 255]);
        let f = flip(&p);
        assert_eq!((at(&f, 0, 0), at(&f, 2, 0), at(&f, 0, 1), at(&f, 1, 1)), (at(&p, 2, 0), at(&p, 0, 0), at(&p, 2, 1), at(&p, 1, 1)));
        assert_eq!(flip(&f).data(), p.data(), "twice is the original");
    }

    #[test]
    fn the_steps_run_in_order_and_an_empty_result_is_none() {
        let src = block();
        let size = |o: Opts| process(&src, &o).map(|p| (p.width(), p.height()));
        assert_eq!(size(Opts { clean: 10, trim: true, flip: false }), Some((4, 4)));
        assert_eq!(size(Opts { clean: 0, trim: true, flip: false }), Some((10, 8)), "an opaque picture has no empty edges until the background goes");
        assert_eq!(size(Opts { clean: 10, trim: false, flip: false }), Some((10, 8)));
        let asym = pic(10, 8, |x, y| if (x, y) == (3, 2) { [0, 255, 0, 255] } else if (3..7).contains(&x) && (2..6).contains(&y) { RED } else { WHITE });
        let flipped = process(&asym, &Opts { clean: 10, trim: true, flip: true }).unwrap();
        assert_eq!(at(&flipped, 3, 0), [0, 255, 0, 255], "the green corner pixel moved from the left to the right");
        assert!(process(&pic(4, 4, |_, _| WHITE), &Opts { clean: 10, trim: true, flip: false }).is_none(), "a picture that is all backdrop leaves nothing");
    }

    /// The pixels of a PNG as straight RGBA, read with the png crate.
    fn decode_rgba(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
        let mut r = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().expect("a valid PNG");
        let mut buf = vec![0; r.output_buffer_size().expect("a size")];
        let info = r.next_frame(&mut buf).expect("its pixels");
        assert_eq!((info.color_type, info.bit_depth), (png::ColorType::Rgba, png::BitDepth::Eight));
        (info.width, info.height, buf[..info.buffer_size()].to_vec())
    }

    #[test]
    fn what_the_creator_writes_reads_back_the_same_through_the_png_crate() {
        // red on the left, half-transparent green on the right, clear at the bottom
        let p = pic(6, 4, |x, y| if y == 3 { NONE } else if x < 3 { RED } else { [0, 100, 0, 200] });
        let (w, h, rgba) = decode_rgba(&encode_png(&p).unwrap());
        assert_eq!((w, h), (6, 4));
        let px = |x: usize, y: usize| rgba[(y * 6 + x) * 4..(y * 6 + x) * 4 + 4].to_vec();
        assert_eq!(px(0, 0), [255, 0, 0, 255], "opaque red");
        assert_eq!(px(0, 3), [0, 0, 0, 0], "clear stays clear");
        assert_eq!(px(5, 0), [0, 128, 0, 200], "partial alpha is stored un-premultiplied: 100 / 200 * 255");
    }

    #[test]
    fn and_the_bar_can_read_it_too() {
        use crate::gfx::Gfx;
        unsafe { let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED); }
        let g = Gfx::new(&crate::conf::pack_dir().join(r"assets\fonts"), &std::env::temp_dir()).unwrap();
        let p = pic(6, 4, |x, y| if y == 3 { NONE } else if x < 3 { RED } else { [0, 100, 0, 200] });
        let back = g.load_image_bytes(&encode_png(&p).unwrap(), 6, 4).expect("WIC reads it");
        assert_eq!(at(&back, 0, 0), RED);
        assert_eq!(at(&back, 0, 3)[3], 0, "transparency survives");
        let g2 = at(&back, 5, 0);
        assert!(g2[3].abs_diff(200) <= 1 && g2[1].abs_diff(100) <= 2, "partial alpha survives the round trip: {g2:?}");
        let big = pic(200, 200, |x, y| [x as u8, y as u8, (x ^ y) as u8, 255]);
        let back = g.load_image_bytes(&encode_png(&big).unwrap(), 200, 200).expect("a larger PNG reads");
        assert_eq!((at(&back, 7, 190), at(&back, 199, 3)), (at(&big, 7, 190), at(&big, 199, 3)));
    }

    #[test]
    fn the_png_is_compressed() {
        // a flat-colour character on a clear background, like most cartoon art
        let art = pic(300, 400, |x, y| if (x as i32 - 150).pow(2) + (y as i32 - 200).pow(2) < 120 * 120 { if y < 150 { RED } else { [200, 80, 20, 255] } } else { NONE });
        let raw = 300 * 400 * 4;
        let small = encode_png(&art).unwrap();
        assert!(small.len() * 20 < raw, "{} bytes for {raw} of pixels", small.len());
        let (w, h, rgba) = decode_rgba(&small);
        assert_eq!((w, h, rgba.len()), (300, 400, raw), "and nothing is lost");
        // incompressible noise stays about the size of the pixels, not far above it
        let mut s = 12345u32;
        let noise = pic(100, 100, |_, _| { s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); [(s >> 8) as u8, (s >> 16) as u8, (s >> 24) as u8, 255] });
        assert!(encode_png(&noise).unwrap().len() < 100 * 100 * 4 + 2000);
    }

    #[test]
    fn names_become_safe_folder_names() {
        assert_eq!(slug("Goku"), "goku");
        assert_eq!(slug("  Son Goku (SSJ) "), "son-goku-ssj");
        assert_eq!(slug("日本語"), "buddy", "nothing usable falls back");
        assert_eq!(slug("a/../b"), "a-b");
        assert_eq!(slug(""), "buddy");
        assert!(slug(&"x".repeat(100)).len() <= 32);
        assert_eq!(slug(&format!("{}-{}", "a".repeat(31), "b")), "a".repeat(31), "a cut that ends on a dash drops it");
        for n in ["Goku", "../../x", "C:\\evil", "My Buddy!!"] { assert!(pack::safe_id(&slug(n)), "{n}"); }
    }

    #[test]
    fn a_second_buddy_of_the_same_name_gets_a_number() {
        let root = std::env::temp_dir().join(format!("zorui-creator-ids-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(unique_id(&root, "Goku"), "goku");
        std::fs::create_dir_all(root.join("goku")).unwrap();
        assert_eq!(unique_id(&root, "Goku"), "goku-2");
        std::fs::create_dir_all(root.join("goku-2")).unwrap();
        assert_eq!(unique_id(&root, "goku"), "goku-3");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_manifest_loads_back_as_the_buddy_that_was_asked_for() {
        let text = manifest("  Son Goku ", 80, "gojo", "Beam");
        let p = pack::parse("goku", Path::new("x"), &text, 1).unwrap();
        assert_eq!((p.name.as_str(), p.height, p.base.as_str(), p.frames["stand"].as_slice()), ("Son Goku", 80.0, "gojo", &["stand.png".to_string()][..]));
        let c = cast::from_pack(&p);
        assert_eq!((c.special, c.label), (cast::Move::Beam, "Son Goku"));
        let odd = pack::parse("x", Path::new("x"), &manifest("N", 9999, "nobody", "laser"), 1).unwrap();
        assert_eq!((odd.height, odd.base.as_str()), (pack::MAX_H, ""), "the height is clamped and an unknown base is neutral");
        assert_eq!(cast::from_pack(&odd).special, cast::Move::Aura, "an unknown move is an aura");
        assert_eq!(pack::parse("x", Path::new("x"), &manifest(&"n".repeat(99), 50, "", "none"), 1).unwrap().name.len(), 40);
    }

    #[test]
    fn saving_makes_a_whole_folder_once_and_never_overwrites() {
        let root = std::env::temp_dir().join(format!("zorui-creator-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let m = manifest("Goku", 64, "", "aura");
        save(&root, "goku", b"PNG", &m).unwrap();
        assert_eq!(std::fs::read(root.join("goku").join("stand.png")).unwrap(), b"PNG");
        assert_eq!(std::fs::read_to_string(root.join("goku").join("buddy.json")).unwrap(), m);
        assert!(!root.join(".goku.tmp").exists(), "no half-made folder is left");
        assert!(save(&root, "goku", b"other", &m).unwrap_err().contains("already"), "an existing buddy is never overwritten");
        assert_eq!(std::fs::read(root.join("goku").join("stand.png")).unwrap(), b"PNG");
        for bad in ["", "..", "a/b", "a\\b"] { assert!(save(&root, bad, b"x", &m).is_err(), "{bad:?}"); }
        assert!(save(&root, "broken", b"x", "{").is_err() && !root.join("broken").exists(), "a manifest that wouldn't load is refused before anything is written");
        let loaded = pack::read(&root, "goku").expect("it is a pack the bar finds");
        assert_eq!(loaded.name, "Goku");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn only_the_creators_own_pictures_may_be_deleted() {
        assert!(is_own_picture("buddy-make-1731111111.png"));
        for no in ["buddy-a-1.png", "wallpaper.jpg", "buddy-make-../x.png", "buddy-make-1.exe", "..\\buddy-make-1.png", ""] { assert!(!is_own_picture(no), "{no:?}"); }
    }

    #[test]
    fn the_preview_is_made_once_per_picture_and_option() {
        use crate::gfx::Gfx;
        unsafe { let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED); }
        let g = Gfx::new(&crate::conf::pack_dir().join(r"assets\fonts"), &std::env::temp_dir()).unwrap();
        let root = std::env::temp_dir().join(format!("zorui-creator-prev-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let src = root.join("in.png");
        std::fs::write(&src, encode_png(&block()).unwrap()).unwrap();
        let dir = root.join("preview");
        let (mut cur, o) = (None, Opts { clean: 10, trim: true, flip: false });
        refresh(&mut cur, &g, &src, &o.key("in.png"), &o, &dir);
        let first = cur.as_ref().unwrap();
        assert!(first.pack.is_some() && first.error.is_empty() && first.stamp == 1, "{}", first.error);
        assert_eq!(first.pack.as_ref().unwrap().frames["stand"], ["stand.png"]);
        let written = std::fs::metadata(dir.join("stand.png")).unwrap().modified().unwrap();
        refresh(&mut cur, &g, &src, &o.key("in.png"), &o, &dir);
        assert_eq!(cur.as_ref().unwrap().stamp, 1, "the same key is not redone");
        assert_eq!(std::fs::metadata(dir.join("stand.png")).unwrap().modified().unwrap(), written);
        let o2 = Opts { flip: true, ..o.clone() };
        refresh(&mut cur, &g, &src, &o2.key("in.png"), &o2, &dir);
        assert_eq!(cur.as_ref().unwrap().stamp, 2, "an option changed, so the stamp moves and the preview reloads its picture");
        let too_much = Opts { clean: 80, trim: true, flip: false };
        std::fs::write(&src, encode_png(&pic(6, 6, |_, _| WHITE)).unwrap()).unwrap();
        refresh(&mut cur, &g, &src, &too_much.key("blank.png"), &too_much, &dir);
        let bad = cur.as_ref().unwrap();
        assert!(bad.pack.is_none() && bad.error.contains("Lower the background"), "{}", bad.error);
        refresh(&mut cur, &g, &root.join("missing.png"), "missing|0|true|false", &Opts::default(), &dir);
        assert!(cur.as_ref().unwrap().error.contains("can't be read"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
