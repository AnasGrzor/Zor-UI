use std::{env, fs, path::PathBuf};

// The icon resource is built by zor-ui's build script (the GNU toolchain here has no windres).
#[path = "../zor-ui/build.rs"]
mod ui;

fn main() {
    ui::main();
    // scripts/build-release.ps1 sets ZORSETUP_ZIP to dist/zorui-app.zip. Without it (cargo test) the payload is empty
    // and the installer reports that instead of installing.
    println!("cargo:rerun-if-env-changed=ZORSETUP_ZIP");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("app.zip");
    match env::var_os("ZORSETUP_ZIP") {
        Some(p) => {
            println!("cargo:rerun-if-changed={}", PathBuf::from(&p).display());
            fs::copy(&p, &out).expect("copy ZORSETUP_ZIP");
        }
        None => fs::write(&out, b"").expect("write empty payload"),
    }
}
