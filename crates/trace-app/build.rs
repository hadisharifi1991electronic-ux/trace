//! Gives the Windows build its icon: compiles assets/trace.rc with windres
//! (MinGW's when cross-compiling) and links the result into the exe.

fn main() {
    println!("cargo:rerun-if-changed=assets/trace.rc");
    println!("cargo:rerun-if-changed=assets/trace.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("trace-res.o");
    let gnu = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu");
    let windres = if gnu && !cfg!(windows) {
        "x86_64-w64-mingw32-windres"
    } else {
        "windres"
    };
    let ok = std::process::Command::new(windres)
        .args([
            "--input",
            "assets/trace.rc",
            "--output-format=coff",
            "--output",
        ])
        .arg(&out)
        .args(["--include-dir", "assets"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        println!("cargo:rustc-link-arg-bins={}", out.display());
    } else {
        println!("cargo:warning=windres not found: the exe is built without its icon");
    }
}
