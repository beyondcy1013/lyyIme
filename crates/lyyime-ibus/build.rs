use std::env;
use std::path::PathBuf;
use std::process::Command;

fn run(cmd: &mut Command) -> Vec<u8> {
    let out = cmd.output().expect("命令执行失败");
    assert!(out.status.success(), "命令失败:{cmd:?}");
    out.stdout
}

fn main() {
    let manifest = env::var("CARGO_MANIFEST_DIR").unwrap();
    let skin_c = PathBuf::from(&manifest).join("../../xim/src/skin.c");
    let skin_h = PathBuf::from(&manifest).join("../../xim/src/skin.h");
    println!("cargo:rerun-if-changed={}", skin_c.display());
    println!("cargo:rerun-if-changed={}", skin_h.display());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let cflags = String::from_utf8(run(Command::new("pkg-config").args([
        "--cflags",
        "gtk+-3.0",
    ])))
    .unwrap();
    let obj = out_dir.join("lyy_skin_gtk.o");
    let mut cc = Command::new(env::var("CC").unwrap_or_else(|_| "cc".into()));
    cc.args(cflags.split_whitespace())
        .arg("-O2")
        .arg("-fPIC")
        .arg("-c")
        .arg(&skin_c)
        .arg("-o")
        .arg(&obj);
    assert!(cc.status().unwrap().success(), "skin.c 编译失败");
    let lib = out_dir.join("liblyy_skin_gtk.a");
    let _ = std::fs::remove_file(&lib);
    assert!(
        Command::new(env::var("AR").unwrap_or_else(|_| "ar".into()))
            .args(["rcs"])
            .arg(&lib)
            .arg(&obj)
            .status()
            .unwrap()
            .success(),
        "ar 打包失败"
    );
    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=lyy_skin_gtk");
}
