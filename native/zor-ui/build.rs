fn main() {
    println!("cargo:rustc-check-cfg=cfg(personal)");
    println!("cargo:rerun-if-env-changed=ZORUI_PUBLIC");
    println!("cargo:rerun-if-changed=src/cast_personal.rs");
    println!("cargo:rerun-if-changed=icon.ico");
    if std::path::Path::new("src/cast_personal.rs").exists() && std::env::var_os("ZORUI_PUBLIC").is_none() {
        println!("cargo:rustc-cfg=personal");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("icon.o");
        std::fs::write(&out, icon_object(&std::fs::read("icon.ico").expect("icon.ico"))).expect("write icon.o");
        println!("cargo:rustc-link-arg-bins={}", out.display());
    }
}

fn u16le(v: u16) -> [u8; 2] { v.to_le_bytes() }
fn u32le(v: u32) -> [u8; 4] { v.to_le_bytes() }

/// A COFF object holding the .rsrc section for `ico`: an RT_GROUP_ICON (id 1, what the exe shows in Explorer and
/// the taskbar) and one RT_ICON per picture. The MSVC and GNU linkers both merge it into the exe's resources;
/// written by hand because the GNU toolchain here has no `windres`.
fn icon_object(ico: &[u8]) -> Vec<u8> {
    let n = u16::from_le_bytes([ico[4], ico[5]]) as usize;
    let mut pics = vec![]; // (directory entry bytes 0..12, picture)
    for i in 0..n {
        let e = &ico[6 + i * 16..6 + i * 16 + 16];
        let (size, off) = (u32::from_le_bytes(e[8..12].try_into().unwrap()) as usize, u32::from_le_bytes(e[12..16].try_into().unwrap()) as usize);
        pics.push((e[..8].to_vec(), ico[off..off + size].to_vec()));
    }
    // resources: (type, id, bytes); RT_ICON = 3, RT_GROUP_ICON = 14
    let mut group = [0u8, 0, 1, 0].to_vec();
    group.extend(u16le(n as u16));
    for (i, (head, pic)) in pics.iter().enumerate() {
        group.extend(&head[..4]);
        group.extend(&head[4..8]); // planes, bit count
        group.extend(u32le(pic.len() as u32));
        group.extend(u16le(i as u16 + 1));
    }
    let mut res: Vec<(u32, u32, Vec<u8>)> = pics.iter().enumerate().map(|(i, p)| (3, i as u32 + 1, p.1.clone())).collect();
    res.push((14, 1, group));

    // layout: root dir, one dir per type, per resource a language dir and a data entry, then the data
    let types = [3u32, 14];
    let root = 16 + 8 * types.len();
    let count = |t: u32| res.iter().filter(|r| r.0 == t).count();
    let mut off = root;
    let type_dir: Vec<usize> = types.iter().map(|&t| { let o = off; off += 16 + 8 * count(t); o }).collect();
    let lang_dir: Vec<usize> = res.iter().map(|_| { let o = off; off += 24; o }).collect();
    let entry: Vec<usize> = res.iter().map(|_| { let o = off; off += 16; o }).collect();
    let mut data_at = vec![];
    let data_start = off;
    for r in &res { data_at.push(off); off += r.2.len().div_ceil(4) * 4; }
    let mut sec = vec![0u8; off];
    let dir = |sec: &mut Vec<u8>, at: usize, ids: usize| { sec[at + 14..at + 16].copy_from_slice(&u16le(ids as u16)); };
    let ent = |sec: &mut Vec<u8>, at: usize, id: u32, target: usize, sub: bool| {
        sec[at..at + 4].copy_from_slice(&u32le(id));
        sec[at + 4..at + 8].copy_from_slice(&u32le(target as u32 | if sub { 0x8000_0000 } else { 0 }));
    };
    dir(&mut sec, 0, types.len());
    for (k, &t) in types.iter().enumerate() {
        ent(&mut sec, 16 + 8 * k, t, type_dir[k], true);
        dir(&mut sec, type_dir[k], count(t));
        let mut j = 0;
        for (i, r) in res.iter().enumerate().filter(|(_, r)| r.0 == t) {
            ent(&mut sec, type_dir[k] + 16 + 8 * j, r.1, lang_dir[i], true);
            j += 1;
            dir(&mut sec, lang_dir[i], 1);
            ent(&mut sec, lang_dir[i] + 16, 0x409, entry[i], false);
        }
    }
    let mut relocs = vec![];
    for (i, r) in res.iter().enumerate() {
        sec[entry[i] + 4..entry[i] + 8].copy_from_slice(&u32le(r.2.len() as u32));
        // the data's RVA is a section-relative address filled in by the linker (addend = offset in the section)
        sec[entry[i]..entry[i] + 4].copy_from_slice(&u32le(data_at[i] as u32));
        relocs.push(entry[i] as u32);
        sec[data_at[i]..data_at[i] + r.2.len()].copy_from_slice(&r.2);
    }
    let _ = data_start;

    let raw = 20 + 40;
    let reloc_at = raw + sec.len();
    let sym_at = reloc_at + relocs.len() * 10;
    let mut o = vec![];
    o.extend(u16le(0x8664)); o.extend(u16le(1)); o.extend(u32le(0)); o.extend(u32le(sym_at as u32)); o.extend(u32le(2)); o.extend(u16le(0)); o.extend(u16le(0));
    o.extend(b".rsrc\0\0\0"); o.extend(u32le(0)); o.extend(u32le(0)); o.extend(u32le(sec.len() as u32)); o.extend(u32le(raw as u32)); o.extend(u32le(reloc_at as u32));
    o.extend(u32le(0)); o.extend(u16le(relocs.len() as u16)); o.extend(u16le(0)); o.extend(u32le(0xC000_0040));
    o.extend(&sec);
    for r in &relocs { o.extend(u32le(*r)); o.extend(u32le(0)); o.extend(u16le(3)); } // IMAGE_REL_AMD64_ADDR32NB against symbol 0
    o.extend(b".rsrc\0\0\0"); o.extend(u32le(0)); o.extend(u16le(1)); o.extend(u16le(0)); o.push(3); o.push(1); // section symbol, static, 1 aux
    o.extend(u32le(sec.len() as u32)); o.extend(u16le(relocs.len() as u16)); o.extend([0u8; 12]); // aux record
    o.extend(u32le(4)); // string table: just its size
    o
}
