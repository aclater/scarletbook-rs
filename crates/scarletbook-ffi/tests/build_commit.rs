// SPDX-License-Identifier: MIT OR Apache-2.0

//! The build script's choice of the commit `sb_build_info` reports (issue #108): only this
//! repository's commits, never a consumer's CI run or a vendoring repository's HEAD.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../build/commit.rs"]
mod commit;

use std::path::Path;

use commit::{Found, REPOSITORY, commit};

const SHA: &str = "027903fb55b02e7854c3c405db915179e3373355";
const OTHER: &str = "1111111111111111111111111111111111111111";

/// A checkout of this repository, as git and Cargo see it.
fn checkout<'a>() -> Found<'a> {
    let root = Path::new("/src/scarletbook-rs");
    Found {
        git_toplevel: Some(root),
        workspace_root: Some(root),
        root_is_ours: true,
        git_head: Some(SHA),
        ..Found::default()
    }
}

#[test]
fn an_explicit_commit_comes_first() {
    let found = Found {
        explicit: Some(OTHER),
        github_sha: Some(SHA),
        github_repository: Some(REPOSITORY),
        ..checkout()
    };
    assert_eq!(commit(&found), OTHER);
    // Not an object name: the next source.
    assert_eq!(commit(&Found { explicit: Some("v1.0 build"), ..checkout() }), SHA);
}

#[test]
fn github_sha_only_in_this_repository() {
    let ours = Found { github_sha: Some(SHA), github_repository: Some(REPOSITORY), ..Found::default() };
    assert_eq!(commit(&ours), SHA);
    // A consumer's CI building this source: its own SHA is not ours.
    for repo in [Some("someone/pysacd"), Some("aclater/scarletbook-rs-fork"), None] {
        let theirs = Found { github_sha: Some(OTHER), github_repository: repo, ..Found::default() };
        assert_eq!(commit(&theirs), "unknown", "{repo:?}");
        // ... falling back on git when the work tree is ours (a fork's checkout).
        let fork = Found { github_sha: Some(OTHER), github_repository: repo, ..checkout() };
        assert_eq!(commit(&fork), SHA, "{repo:?}");
    }
}

#[test]
fn git_only_in_this_workspace() {
    assert_eq!(commit(&checkout()), SHA);
    // Vendored into another repository: git's work tree is theirs.
    let vendored = Found { git_toplevel: Some(Path::new("/src/pysacd")), ..checkout() };
    assert_eq!(commit(&vendored), "unknown");
    // The same root but without this crate in it, or either path unknown.
    assert_eq!(commit(&Found { root_is_ours: false, ..checkout() }), "unknown");
    assert_eq!(commit(&Found { git_toplevel: None, ..checkout() }), "unknown");
    assert_eq!(commit(&Found { workspace_root: None, git_toplevel: None, ..checkout() }), "unknown");
    // No git at all, nothing set.
    assert_eq!(commit(&Found::default()), "unknown");
    assert_eq!(commit(&Found { git_head: Some("not-a-sha"), ..checkout() }), "unknown");
    assert_eq!(commit(&Found { git_head: Some(&SHA[..7]), ..checkout() }), &SHA[..7]);
}
