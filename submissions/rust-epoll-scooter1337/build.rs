fn main() {
    println!("cargo:rerun-if-changed=vendor/sqlite3.c");
    println!("cargo:rerun-if-changed=bench/train_sqlite.c");
    let mut build=cc::Build::new();
    let native=std::env::var("HOST").unwrap()==std::env::var("TARGET").unwrap();
    if native {
        build.flag_if_supported("-march=native");
    }
    build.file("vendor/sqlite3.c").opt_level(3)
        .define("SQLITE_THREADSAFE", "0")
        .define("SQLITE_DEFAULT_MEMSTATUS", "0")
        .define("SQLITE_DQS", "0")
        .define("SQLITE_OMIT_LOAD_EXTENSION", None)
        .define("SQLITE_OMIT_DEPRECATED", None)
        .define("SQLITE_OMIT_SHARED_CACHE", None)
        .define("SQLITE_OMIT_PROGRESS_CALLBACK", None)
        .define("SQLITE_LIKE_DOESNT_MATCH_BLOBS", None)
        .define("SQLITE_USE_ALLOCA", None);
    // SQLite's Linux VFS supports fdatasync with the same WAL/NORMAL guarantees.
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap() == "linux" {
        build.define("HAVE_FDATASYNC", "1");
    }
    let compiler=build.get_compiler();
    if native && compiler.is_like_gnu() && !compiler.is_like_clang() && std::env::var_os("CARGO_FEATURE_SQLITE_PGO").is_some() {
        use std::{path::PathBuf,process::Command};
        fn run(cmd:&mut Command) { assert!(cmd.status().expect("run PGO compiler/trainer").success(),"PGO command failed: {cmd:?}"); }
        let out=PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
        let profile=out.join("sqlite-profile");std::fs::create_dir_all(&profile).unwrap();
        let object=out.join("sqlite-pgo.o");let trainer=out.join("sqlite-train");
        let generate=format!("-fprofile-generate={}",profile.display());
        run(compiler.to_command().arg(&generate).arg("-c").arg("vendor/sqlite3.c").arg("-o").arg(&object));
        run(compiler.to_command().arg(&generate).arg("-Ivendor").arg("bench/train_sqlite.c").arg(&object).arg("-lm").arg("-o").arg(&trainer));
        run(Command::new(&trainer).arg(out.join("training.db")));
        run(compiler.to_command().arg(format!("-fprofile-use={}",profile.display())).arg("-fprofile-correction").arg("-c").arg("vendor/sqlite3.c").arg("-o").arg(&object));
        run(build.get_archiver().arg("crs").arg(out.join("libsqlite3.a")).arg(&object));
        println!("cargo:rustc-link-search=native={}",out.display());
        println!("cargo:rustc-link-lib=static=sqlite3");
    } else { build.compile("sqlite3"); }
    println!("cargo:rustc-link-lib=m");
}
