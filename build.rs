//! Works out the version from git tags, and embeds the exe icon and version info (shown in
//! the file's Properties) on Windows. There's no version number to edit for a release:
//! tagging `v1.2.3` is the release.

use std::path::PathBuf;
use std::process::Command;

#[path = "src/git_version.rs"]
#[allow(dead_code)]
mod git_version;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The version from `git describe`, or Cargo.toml's placeholder outside a git checkout.
fn version() -> String {
    // Rebuild when HEAD moves or tags change (paths work in worktrees too).
    if let (Some(git_dir), Some(common)) = (
        git(&["rev-parse", "--absolute-git-dir"]),
        git(&["rev-parse", "--path-format=absolute", "--git-common-dir"]),
    ) {
        for p in [format!("{git_dir}/HEAD"), format!("{git_dir}/logs/HEAD")] {
            println!("cargo:rerun-if-changed={p}");
        }
        for p in [format!("{common}/packed-refs"), format!("{common}/refs/tags")] {
            println!("cargo:rerun-if-changed={p}");
        }
    }
    let describe = git(&["describe", "--tags", "--long", "--match", "v[0-9]*", "--exclude", "*-dev.*"]);
    match describe.as_deref().and_then(git_version::from_describe) {
        Some(v) => v,
        None => {
            println!("cargo:warning=no version tag found in git; using Cargo.toml's version");
            std::env::var("CARGO_PKG_VERSION").unwrap()
        }
    }
}

fn main() {
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=src/git_version.rs");

    let version = version();
    println!("cargo:rustc-env=NTFY_TOAST_VERSION={version}");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    // CI reads this to name the release.
    std::fs::write(out_dir.join("version.txt"), &version).unwrap();

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let commas = git_version::file_version(&version).map(|n| n.to_string()).join(",");
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let icon = manifest_dir.join("assets").join("icon.ico");
    let rc = std::fs::read_to_string(manifest_dir.join("assets/app.rc"))
        .unwrap()
        .replace("@VERSION_COMMAS@", &commas)
        .replace("@VERSION@", &version)
        .replace("@ICON@", &icon.display().to_string().replace('\\', "/"));

    let out = out_dir.join("app.rc");
    std::fs::write(&out, rc).unwrap();
    embed_resource::compile(&out, embed_resource::NONE);
}
