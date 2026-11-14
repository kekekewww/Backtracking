use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=src/hotkey.c");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }

    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let object = out.join("hotkey.o");
    let archive = out.join("libbacktracking_hotkey.a");
    let pkg_config = env::var_os("PKG_CONFIG").unwrap_or_else(|| "pkg-config".into());
    let flags = |option| {
        let result = Command::new(&pkg_config)
            .args([option, "x11", "gio-2.0"])
            .output()
            .expect("could not run pkg-config");
        assert!(
            result.status.success(),
            "Install the X11 and GLib development libraries. {}",
            String::from_utf8_lossy(&result.stderr)
        );
        String::from_utf8(result.stdout).expect("pkg-config returned non-UTF-8 flags")
    };

    let cc = env::var_os("CC").unwrap_or_else(|| "cc".into());
    let status = Command::new(cc)
        .args(["-std=c11", "-Wall", "-Wextra", "-O2", "-c", "src/hotkey.c", "-o"])
        .arg(&object)
        .args(flags("--cflags").split_ascii_whitespace())
        .status()
        .expect("could not run C compiler");
    assert!(status.success(), "could not compile Linux shortcut backend");

    let ar = env::var_os("AR").unwrap_or_else(|| "ar".into());
    let status = Command::new(ar)
        .arg("crs")
        .arg(&archive)
        .arg(&object)
        .status()
        .expect("could not run archiver");
    assert!(status.success(), "could not archive Linux shortcut backend");

    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=backtracking_hotkey");
    for flag in flags("--libs").split_ascii_whitespace() {
        if let Some(library) = flag.strip_prefix("-l") {
            println!("cargo:rustc-link-lib={library}");
        } else if let Some(directory) = flag.strip_prefix("-L") {
            println!("cargo:rustc-link-search=native={directory}");
        } else {
            println!("cargo:rustc-link-arg={flag}");
        }
    }
}
