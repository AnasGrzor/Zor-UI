fn main() {
    println!("cargo:rustc-check-cfg=cfg(personal)");
    println!("cargo:rerun-if-env-changed=ZORUI_PUBLIC");
    println!("cargo:rerun-if-changed=src/cast_personal.rs");
    if std::path::Path::new("src/cast_personal.rs").exists() && std::env::var_os("ZORUI_PUBLIC").is_none() {
        println!("cargo:rustc-cfg=personal");
    }
}
