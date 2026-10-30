// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! This plane is the same plane everywhere it is compiled.
//!
//! Mirrors the sibling planes' own `invariance.rs` (decisions, mcp, a2a). No features, no
//! conditionally compiled item, the two lint gates, no `unsafe` block, a manifest that names no
//! kernel-side crate and no sibling plane, and comments that cite the design in words.
//! `the_manifest_names_only_what_this_plane_may_name` must never be relaxed to keep it green; if
//! `busbar-kernel` reaches the manifest, that is the line that is supposed to fail.

use std::path::{Path, PathBuf};

fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn manifest() -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
        .expect("the manifest is readable")
}

fn walk(dir: &Path, f: &mut impl FnMut(&Path, &str)) {
    for entry in std::fs::read_dir(dir)
        .expect("the source directory is readable")
        .flatten()
    {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, f);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let text = std::fs::read_to_string(&path).expect("a source file is readable");
            f(&path, &text);
        }
    }
}

/// The manifest declares no features at all.
#[test]
fn the_crate_declares_no_features() {
    let manifest = manifest();
    assert!(
        !manifest.contains("[features]"),
        "the plane declares cargo features, so it is not one plane"
    );
    assert!(
        !manifest.contains("optional = true"),
        "an optional dependency is a feature by another name"
    );
}

/// No item on the surface is behind a conditional-compilation attribute (test modules exempted).
#[test]
fn no_item_is_conditionally_compiled() {
    let mut offenders = Vec::new();
    walk(&src_dir(), &mut |path, text| {
        for (n, line) in text.lines().enumerate() {
            let line = line.trim_start();
            if line.starts_with("#[cfg(test)]") {
                continue;
            }
            if line.starts_with("#[cfg(") || line.starts_with("#![cfg(") {
                offenders.push(format!("{}:{}", path.display(), n + 1));
            }
        }
    });
    assert!(
        offenders.is_empty(),
        "conditionally compiled items: {offenders:?}"
    );
}

/// The crate carries the two lint gates the design requires of a plugin.
#[test]
fn the_crate_forbids_unsafe_and_undocumented_items() {
    let lib = std::fs::read_to_string(src_dir().join("lib.rs")).expect("the root is readable");
    assert!(lib.contains("#![forbid(unsafe_code)]"));
    assert!(lib.contains("#![deny(missing_docs)]"));
}

/// No source line outside a comment opens an `unsafe` block or item.
#[test]
fn the_source_contains_no_unsafe_block() {
    let mut offenders = Vec::new();
    walk(&src_dir(), &mut |path, text| {
        for (n, line) in text.lines().enumerate() {
            let t = line.trim_start();
            if t.starts_with("//") {
                continue;
            }
            if line.contains("unsafe ") {
                offenders.push(format!("{}:{}", path.display(), n + 1));
            }
        }
    });
    assert!(
        offenders.is_empty(),
        "unsafe code on the surface: {offenders:?}"
    );
}

/// The manifest names no kernel-side crate, never the legacy plugin-facing one, and never a sibling
/// plane (a plane reading another plane's private surface is never legitimate). Siblings are
/// forbidden by shape (`busbar-plane-*` other than this crate), not by name.
#[test]
fn the_manifest_names_only_what_this_plane_may_name() {
    let manifest = manifest();
    let deps = manifest
        .split("[dependencies]")
        .nth(1)
        .expect("the manifest has a dependency section");
    let deps = deps.split("\n[").next().unwrap_or_default();
    for line in deps.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let name = line.split_whitespace().next().unwrap_or_default();
        for forbidden in [
            "busbar-api",
            "busbar-core",
            "busbar-caps",
            "busbar-kernel",
            "busbar-unit-",
            "busbar-admin",
            "busbar-plugin-loader",
        ] {
            assert!(
                !name.starts_with(forbidden),
                "the plane depends on {name}, which is kernel-side"
            );
        }
        assert!(
            !name.starts_with("busbar-plane-") || name == env!("CARGO_PKG_NAME"),
            "the plane depends on {name}, a sibling plane"
        );
    }
}

/// The comments cite the design in words, not in section numbers.
#[test]
fn the_comments_cite_the_design_in_words() {
    let mut offenders = Vec::new();
    let mut check = |path: &Path, text: &str| {
        for (n, line) in text.lines().enumerate() {
            if line.contains('\u{a7}') {
                offenders.push(format!("{}:{}: section sign", path.display(), n + 1));
            }
        }
    };
    walk(&src_dir(), &mut check);
    walk(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests"),
        &mut check,
    );
    assert!(
        offenders.is_empty(),
        "the source cites the design by section-sign rather than in words: {offenders:?}"
    );
}
