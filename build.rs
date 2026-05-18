//! Link `libbitcoinkernel` when building with `--features bitcoinkernel`.
//!
//! Set **`BITCOIN_CORE_LIB_DIR`** to the directory containing **`libbitcoinkernel.so`**
//! or **`libbitcoinkernel.a`** (CMake default is often the static archive under `build/lib/`).
//! Alternatively ensure **`pkg-config`** can find **`libbitcoinkernel`**.

use std::path::Path;

fn main() {
    if std::env::var_os("CARGO_FEATURE_BITCOINKERNEL").is_none() {
        return;
    }

    println!("cargo:rerun-if-env-changed=BITCOIN_CORE_LIB_DIR");
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");

    if let Ok(dir) = std::env::var("BITCOIN_CORE_LIB_DIR") {
        if !dir.is_empty() {
            println!("cargo:rustc-link-search=native={dir}");
            let p = Path::new(&dir);
            if p.join("libbitcoinkernel.so").exists() || p.join("libbitcoinkernel.dylib").exists() {
                println!("cargo:rustc-link-lib=dylib=bitcoinkernel");
            } else if p.join("libbitcoinkernel.a").exists() {
                println!("cargo:rustc-link-lib=static=bitcoinkernel");
                println!("cargo:rustc-link-lib=dylib=stdc++");
            } else {
                panic!(
                    "BITCOIN_CORE_LIB_DIR={dir}: expected libbitcoinkernel.so or libbitcoinkernel.a"
                );
            }
            return;
        }
    }

    if let Ok(out) = std::process::Command::new("pkg-config")
        .args(["--libs", "libbitcoinkernel"])
        .output()
    {
        if out.status.success() {
            let stdout = String::from_utf8_lossy(&out.stdout);
            for token in stdout.split_whitespace() {
                if let Some(p) = token.strip_prefix("-L") {
                    println!("cargo:rustc-link-search=native={p}");
                } else if let Some(lib) = token.strip_prefix("-l") {
                    println!("cargo:rustc-link-lib=dylib={lib}");
                }
            }
            return;
        }
    }

    panic!(
        "feature `bitcoinkernel`: set BITCOIN_CORE_LIB_DIR to the directory containing \
         libbitcoinkernel.so or libbitcoinkernel.a, or add libbitcoinkernel.pc to PKG_CONFIG_PATH"
    );
}
