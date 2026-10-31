//! Purity gates, checked here rather than asserted in prose: the plane names no kernel-side crate,
//! holds no tool-executor port, and intercepts no reply. (The plane does keep state across calls by
//! design, its sessions and units behind locks; no gate here claims otherwise.)

// ── THE KERNEL-SIDE FORBID LIST ───────────────────────────────────────────────────────────────
//
// Its four sibling planes (`busbar-plane-a2a`, `-mcp`, `-llm`, `-decision`) each carry this test.
// THIS CRATE DID NOT, and the asymmetry was invisible because the rule it asserts was green
// anyway — so nothing pointed at the gap, and a reach landing here would have been the first
// thing to notice it. The manifest allow-list is the real control; this is the same rule asserted
// from the INSIDE, so a reach for the kernel is a red here rather than a discovery at packaging.

use std::path::{Path, PathBuf};

fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Walk every source file, handing each to a reader.
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

/// A line that is only a comment says nothing about what the code does.
///
/// LOAD-BEARING HERE, not decoration: this crate's headers legitimately DISCUSS the kernel and its
/// sibling planes — `lib.rs:35` names the older `busbar_kernel::plane::registry::PlaneDecl`
/// architecture it is not built on and `claims.rs:117` cites `busbar_plane_llm`'s ladder. Every
/// one is prose about what this plane does NOT do. A gate its own explanation fails is a gate
/// somebody deletes.
fn is_comment(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("//") || t.starts_with('*') || t.starts_with("/*")
}

/// The plane names no kernel-side crate, and no sibling plane.
///
/// The legacy engine's path is matched with its `::` on purpose: the bare stem would also match
/// identifiers in this crate's own `codec` module's doc comments, which this scan skips as
/// comments — banning it would ban the thing the plane is built out of. The path separator is what
/// distinguishes reaching into the legacy ENGINE from using the codec.
#[test]
fn the_plane_names_no_kernel_side_crate() {
    // The list is data (`tests/fixtures/forbidden_crates.txt`): the sibling planes' crate names are
    // what the scan looks FOR, and a plane's own source names no sibling.
    let forbidden: Vec<&str> = include_str!("fixtures/forbidden_crates.txt")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    assert_eq!(
        forbidden.len(),
        9,
        "the forbidden-crate fixture lost or gained a row"
    );
    let mut offenders = Vec::new();
    walk(&src_dir(), &mut |path, text| {
        for (n, line) in text.lines().enumerate() {
            if is_comment(line) {
                continue;
            }
            for name in &forbidden {
                if line.contains(name) {
                    offenders.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
                }
            }
        }
    });
    assert!(
        offenders.is_empty(),
        "the streaming plane reaches a kernel-side crate or a sibling plane: {offenders:#?}"
    );
}

/// LAW 11 (QUESTIONS Q98): busbar does not act on the data it carries, so the plane holds no port
/// that runs a tool. A tool call is relayed to the caller as-is (`session_pump`'s
/// `a_tool_call_is_relayed_to_the_caller_as_is_and_the_gateway_answers_nothing`); this arm keeps the
/// executor seam itself from coming back. RED: `tools.rs` declared `ToolExecutor` with an
/// `async fn execute(&self, name, arguments) -> Vec<u8>` the session pump called on a call's close.
#[test]
fn the_plane_holds_no_tool_executor_port() {
    let mut offenders = Vec::new();
    walk(&src_dir(), &mut |path, text| {
        for (n, line) in text.lines().enumerate() {
            if is_comment(line) {
                continue;
            }
            if line.contains("ToolExecutor") || line.contains("fn execute(") {
                offenders.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
            }
        }
    });
    assert!(
        offenders.is_empty(),
        "the streaming plane runs no tool; a tool call is the caller's to run:\n{}",
        offenders.join("\n")
    );
}

/// RED-BEFORE-GREEN (Law 11, "Code that answers, executes or branches on what a request or response
/// asks for is a defect: it is deleted, never added"): the plane holds no table that refuses a
/// caller's tool reply by its call id and no seam that authors a `response.create` after one.
/// RED: `governed.rs` and `open_calls.rs` declared `GovernedCalls` and `OpenToolCalls`, and the
/// session pump intercepted a caller's reply through them.
#[test]
fn the_plane_holds_no_reply_interception() {
    let mut offenders = Vec::new();
    walk(&src_dir(), &mut |path, text| {
        for (n, line) in text.lines().enumerate() {
            if is_comment(line) {
                continue;
            }
            if line.contains("GovernedCalls") || line.contains("OpenToolCalls") {
                offenders.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
            }
        }
    });
    assert!(
        offenders.is_empty(),
        "a caller's tool reply is the caller's; the plane intercepts none:\n{}",
        offenders.join("\n")
    );
}
