#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# The voice battery's subject, built from THIS repo. Usage: voice-overlay.sh <repo root> <dest dir>.
#
# Writes, at run time and never into a committed manifest, a cargo workspace under <dest> that holds
# this repo's own plane, its cdylib twin and its own copy of the battery's harness
# (testing/voice-conformance/harness, which is not a member of the root workspace). The harness
# reaches into the plane's codecs, claims and driven tables, which the plane's crate root keeps
# private (`mod`) or test-only (`#[cfg(test)]`) on purpose, so the COPY of the plane in <dest> is
# opened up: `mod x;` becomes `pub mod x;` and the `#[cfg(test)]` that fences an item the harness
# reads is dropped. The product tree is never touched, and the copy changes visibility only, no
# behaviour: the door the harness drives is the door the plane ships.
set -euo pipefail
root="$1"; dest="$2"
rm -rf "$dest"; mkdir -p "$dest/testing/voice-conformance"
cp "$root/Cargo.toml" "$root/Cargo.lock" "$root/rust-toolchain.toml" "$dest/"
cp -R "$root/plane-streaming" "$root/plane-streaming-plugin" "$dest/"
cp -R "$root/testing/voice-conformance/harness" "$dest/testing/voice-conformance/"
rm -rf "$dest"/plane-streaming/target "$dest"/plane-streaming-plugin/target
sed -i.bak 's#^    "plane-streaming-plugin",#    "plane-streaming-plugin",\n    "testing/voice-conformance/harness",#' "$dest/Cargo.toml"
sed -i.bak 's#^\[workspace.dependencies\]#[workspace.dependencies]\nbusbar-plane-streaming = { path = "plane-streaming" }\nbusbar-plane-streaming-plugin = { path = "plane-streaming-plugin" }#' "$dest/Cargo.toml"
src="$dest/plane-streaming/src"
sed -i.bak -e 's/^mod \([a-z_]*\);/pub mod \1;/' -e 's/^#!\[deny(missing_docs)\]//' "$src/lib.rs"
# the fence on an item is dropped; the fence on each of the plane's own unit-test modules stays
# (an out-of-line `mod <name>;`, after any `#[path = ...]`: they read the contract's test-only SDK
# and `crate::tests`), and the tests directory is not touched. `voice.sh --selftest` holds this.
find "$src" -name '*.rs' ! -path "$src/tests/*" -print0 \
  | xargs -0 perl -0pi -e 's/^[ \t]*#\[cfg\(test\)\][ \t]*\n(?!(?:[ \t]*#\[[^\n]*\n)*[ \t]*(pub(\([a-z]+\))? )?mod [a-z_0-9]+;)//mg'
find "$dest" -name '*.bak' -delete
