// SPDX-License-Identifier: Apache-2.0
//! The actual artifact supplied by the build lane, never a linked substitute.
pub fn plugin_cdylib() -> std::path::PathBuf {
    let path = std::path::PathBuf::from(
        std::env::var_os("BUSBAR_PLUGIN_CDYLIB")
            .expect("build this actual plugin and set BUSBAR_PLUGIN_CDYLIB"),
    );
    assert!(path.is_absolute(), "BUSBAR_PLUGIN_CDYLIB must be absolute");
    assert!(
        path.is_file(),
        "actual streaming cdylib is missing: {}",
        path.display()
    );
    let built = std::fs::metadata(&path).unwrap().modified().unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    fn newest(dir: &std::path::Path) -> std::time::SystemTime {
        let mut time = std::time::SystemTime::UNIX_EPOCH;
        for entry in std::fs::read_dir(dir).expect("source directory") {
            let entry = entry.unwrap();
            let p = entry.path();
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
            if stem == "tests" || stem.starts_with("tests_") {
                continue;
            }
            if p.is_dir() {
                time = time.max(newest(&p));
            } else if p.extension().is_some_and(|e| e == "rs") {
                time = time.max(entry.metadata().unwrap().modified().unwrap());
            }
        }
        time
    }
    for member in ["plane-streaming", "plane-streaming-plugin"] {
        assert!(
            built >= newest(&root.join(member).join("src")),
            "stale actual streaming cdylib: {}",
            path.display()
        );
    }
    path
}
