use std::path::{Path, PathBuf};
use std::{env, fs};

// Copy assets/ next to the built binary so asset_uri() (exe_dir/assets/...) resolves.
fn main() {
    println!("cargo:rerun-if-changed=assets");

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let src = manifest.join("assets");

    // OUT_DIR = target/<profile>/build/<pkg>/out -> exe dir is three parents up.
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let Some(exe_dir) = out.ancestors().nth(3) else {
        return;
    };
    let dst = exe_dir.join("assets");

    copy_dir(&src, &dst);
}

fn copy_dir(src: &Path, dst: &Path) {
    let _ = fs::create_dir_all(dst);
    let Ok(entries) = fs::read_dir(src) else {
        return;
    };
    for entry in entries.flatten() {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to);
        } else {
            let _ = fs::copy(&from, &to);
        }
    }
}
