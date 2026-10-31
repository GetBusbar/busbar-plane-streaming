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

/// The sibling planes, by their crate names (lead ruling: a sibling check names the fixed list, never
/// a directory scan, which is vacuous in a standalone repo), the cdylib twins with them.
const SIBLING_PLANES: &[&str] = &[
    "busbar-plane-llm",
    "busbar-plane-mcp",
    "busbar-plane-a2a",
    "busbar-plane-decisions",
];

/// The crates a plane never depends on: the kernel side, and the legacy plugin-facing loader.
const KERNEL_SIDE: &[&str] = &[
    "busbar-api",
    "busbar-core",
    "busbar-caps",
    "busbar-kernel",
    "busbar-unit-",
    "busbar-admin",
    "busbar-plugin-loader",
];

/// Every crate `manifest`'s shipped dependency tables name: `[dependencies]`,
/// `[build-dependencies]`, every `[target.<cfg>.…]` twin of them, and each `[dependencies.<name>]`
/// table; by its key and by any `package = "…"` rename. (Dev-dependencies are not shipped.)
fn shipped_dependencies(manifest: &str) -> Vec<String> {
    let shipped = |table: &str| {
        let last = table.rsplit('.').next().unwrap_or(table);
        let head = table.split('.').next().unwrap_or(table);
        (last == "dependencies" || last == "build-dependencies")
            || (head == "dependencies" || head == "build-dependencies")
            || (table.starts_with("target.")
                && (table.contains(".dependencies") || table.contains(".build-dependencies")))
    };
    let unquote = |v: &str| v.trim().trim_matches('"').to_string();
    let mut names = Vec::new();
    let mut table = String::new();
    for line in manifest.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        if let Some(header) = line.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
            table = header.trim().to_string();
            if !shipped(&table) {
                continue;
            }
            // `[dependencies.<name>]` (or a target's) names its crate in the header.
            for marker in ["dependencies.", "build-dependencies."] {
                if let Some((_, name)) = table.rsplit_once(marker) {
                    if !name.contains('.') {
                        names.push(unquote(name));
                    }
                }
            }
            continue;
        }
        if !shipped(&table) {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = unquote(key);
        let in_dep_table = table.ends_with("dependencies");
        if in_dep_table {
            names.push(key.clone());
        }
        // A rename: `package = "…"`, inline or in the crate's own table.
        let package = if key == "package" {
            Some(unquote(value))
        } else {
            value
                .split_once("package")
                .and_then(|(_, rest)| rest.split_once('='))
                .map(|(_, rest)| unquote(rest.split([',', '}']).next().unwrap_or_default()))
        };
        names.extend(package);
    }
    names
}

/// What in `manifest` this plane may not name: a kernel-side crate, or a sibling plane.
fn forbidden_dependencies(manifest: &str) -> Vec<String> {
    shipped_dependencies(manifest)
        .into_iter()
        .filter(|name| {
            KERNEL_SIDE.iter().any(|k| name.starts_with(k))
                || SIBLING_PLANES
                    .iter()
                    .any(|s| name == s || name.strip_prefix(s) == Some("-plugin"))
                || (name.starts_with("busbar-plane-") && name != env!("CARGO_PKG_NAME"))
        })
        .collect()
}

/// The manifest names no kernel-side crate, never the legacy plugin-facing one, and never a sibling
/// plane (a plane reading another plane's private surface is never legitimate).
#[test]
fn the_manifest_names_only_what_this_plane_may_name() {
    let manifest = manifest();
    assert!(
        manifest.contains("[dependencies]"),
        "the manifest has a dependency section"
    );
    assert_eq!(forbidden_dependencies(&manifest), Vec::<String>::new());
}

/// The check reads every shipped dependency table and every rename (audit S88): a kernel-side crate
/// or a sibling named in a `[dependencies.<name>]` table, a target's table, or under another key
/// through `package` is refused; the inline `[dependencies]` line was the only form read before.
#[test]
fn a_forbidden_crate_is_found_in_every_form_a_manifest_can_name_it() {
    for (manifest, named) in [
        (
            "[dependencies.busbar-kernel]\ngit = \"x\"\n",
            "busbar-kernel",
        ),
        (
            "[target.'cfg(unix)'.dependencies]\nbusbar-core = \"1\"\n",
            "busbar-core",
        ),
        (
            "[dependencies]\nx = { package = \"busbar-plane-llm\", version = \"1\" }\n",
            "busbar-plane-llm",
        ),
        (
            "[dependencies.x]\npackage = \"busbar-plane-mcp\"\n",
            "busbar-plane-mcp",
        ),
        (
            "[build-dependencies]\nbusbar-plugin-loader = \"1\"\n",
            "busbar-plugin-loader",
        ),
    ] {
        assert!(
            forbidden_dependencies(manifest).iter().any(|n| n == named),
            "{manifest}"
        );
    }
    let fine = "[dependencies]\nbusbar-contract = { workspace = true }\n\n[dev-dependencies]\nbusbar-plugin-loader = \"1\"\n";
    assert_eq!(forbidden_dependencies(fine), Vec::<String>::new());
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
