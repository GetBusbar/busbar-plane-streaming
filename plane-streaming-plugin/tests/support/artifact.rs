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
    // The build lane binds the exact source and fresh library hash. Filesystem timestamps do
    // not establish that identity and can change when an immutable source archive is unpacked.
    path
}
