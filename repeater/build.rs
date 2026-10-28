use std::{env, path::Path};

fn main() {
    println!("cargo:rustc-check-cfg=cfg(mcrs_profile)");
    println!("cargo:rerun-if-env-changed=MCRS_PROFILE");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../profiles/defaults.conf");
    let manifest = env::var("CARGO_MANIFEST_DIR").unwrap();
    let workspace = Path::new(&manifest).parent().unwrap();
    if let Some(selected) = env::var_os("MCRS_PROFILE").filter(|value| !value.is_empty()) {
        let file = Path::new(&selected);
        // Bare filenames refer to the profiles directory. Explicit paths are
        // resolved from the workspace root (absolute paths work unchanged).
        let path = if file.components().count() == 1 {
            workspace.join("profiles").join(file)
        } else {
            workspace.join(file)
        };
        println!("cargo:rerun-if-changed={}", path.display());
        let path = path.canonicalize().unwrap_or_else(|error| {
            panic!(
                "PROFILE must name a single file; cannot open {}: {error}",
                path.display()
            )
        });
        println!("cargo:rerun-if-changed={}", path.display());
        let path = path.to_str().expect("profile path must be valid UTF-8");
        assert!(
            !path.contains(['\n', '\r']),
            "profile path must not contain newlines"
        );
        println!("cargo:rustc-env=MCRS_PROFILE_PATH={path}");
        println!("cargo:rustc-cfg=mcrs_profile");
    }
}
