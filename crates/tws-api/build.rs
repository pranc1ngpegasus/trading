fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-env-changed=TWS_API_ROOT");
    let root = std::env::var("TWS_API_ROOT").map_err(|_| {
        "TWS_API_ROOT is unset. Run inside `nix develop`, or point it at an unpacked TWS API download."
    })?;

    let proto_dir = format!("{root}/source/proto");
    let entries = std::fs::read_dir(&proto_dir)
        .map_err(|err| format!("cannot read {proto_dir}: {err}; is TWS_API_ROOT correct?"))?;
    let mut files = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "proto") {
            files.push(path.to_string_lossy().into_owned());
        }
    }
    if files.is_empty() {
        return Err(format!("no .proto files under {proto_dir}").into());
    }
    files.sort();

    let version = std::fs::read_to_string(format!("{root}/API_VersionNum.txt"))?;
    let version = version.trim().trim_start_matches("API_Version=");
    println!("cargo:rustc-env=TWS_API_VERSION={version}");

    buffa_build::Config::new()
        .files(&files)
        .includes(&[proto_dir.as_str()])
        .include_file("_include.rs")
        .generate_views(false)
        .idiomatic_field_names(true)
        .compile()?;
    Ok(())
}
