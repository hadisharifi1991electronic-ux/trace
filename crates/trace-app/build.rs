//! Gives the Windows build its icon: compiles assets/trace.rc with windres
//! (MinGW's when cross-compiling) and links the result into the exe.

fn main() {
    println!("cargo:rerun-if-changed=assets/trace.rc");
    println!("cargo:rerun-if-changed=assets/trace.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let out = dir.join("trace-res.o");
    // The icon plus version details shown in the exe's Properties.
    let v = std::env::var("CARGO_PKG_VERSION").unwrap();
    let mut n: Vec<u32> = v.split('.').map(|p| p.parse().unwrap_or(0)).collect();
    n.resize(4, 0);
    let rc = dir.join("trace.rc");
    std::fs::write(
        &rc,
        format!(
            "1 ICON \"trace.ico\"\n\
1 VERSIONINFO\nFILEVERSION {a},{b},{c},{d}\nPRODUCTVERSION {a},{b},{c},{d}\n\
BEGIN\n BLOCK \"StringFileInfo\"\n BEGIN\n  BLOCK \"040904B0\"\n  BEGIN\n\
   VALUE \"FileDescription\", \"Trace PCB design tool\"\n\
   VALUE \"ProductName\", \"Trace\"\n\
   VALUE \"FileVersion\", \"{v}\"\n\
   VALUE \"ProductVersion\", \"{v}\"\n\
   VALUE \"LegalCopyright\", \"GPL-3.0-or-later\"\n\
  END\n END\n BLOCK \"VarFileInfo\"\n BEGIN\n  VALUE \"Translation\", 0x409, 1200\n END\nEND\n",
            a = n[0],
            b = n[1],
            c = n[2],
            d = n[3]
        ),
    )
    .unwrap();
    let gnu = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu");
    let windres = if gnu && !cfg!(windows) {
        "x86_64-w64-mingw32-windres"
    } else {
        "windres"
    };
    let ok = std::process::Command::new(windres)
        .args([
            "--input",
            rc.to_str().unwrap(),
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
