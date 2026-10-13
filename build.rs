//! Embeds the exe icon and version info (shown in the file's Properties) on Windows.
//! The version comes from Cargo.toml, so there's nothing to edit here for a release.

fn main() {
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    // FILEVERSION needs four numbers; drop any pre-release suffix like "-beta.1".
    let mut parts: Vec<String> = version
        .split(['.', '-', '+'])
        .take(3)
        .map(|p| p.parse::<u16>().unwrap_or(0).to_string())
        .collect();
    parts.resize(4, "0".into());

    let manifest_dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let icon = manifest_dir.join("assets").join("icon.ico");
    let rc = std::fs::read_to_string(manifest_dir.join("assets/app.rc"))
        .unwrap()
        .replace("@VERSION_COMMAS@", &parts.join(","))
        .replace("@VERSION@", &version)
        .replace("@ICON@", &icon.display().to_string().replace('\\', "/"));

    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("app.rc");
    std::fs::write(&out, rc).unwrap();
    embed_resource::compile(&out, embed_resource::NONE);
}
