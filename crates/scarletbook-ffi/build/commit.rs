// SPDX-License-Identifier: MIT OR Apache-2.0

//! Which commit `sb_build_info` reports (issue #108): a pure decision over what the build
//! script found, so that it can be tested (`tests/build_commit.rs`). The build script does
//! the I/O (environment, git) and hands the results here.

use std::path::Path;

/// The repository whose commits are reported.
pub const REPOSITORY: &str = "aclater/scarletbook-rs";

/// What the build script found.
#[derive(Debug, Default)]
pub struct Found<'a> {
    /// `SCARLETBOOK_COMMIT`: set explicitly by whoever builds this library.
    pub explicit: Option<&'a str>,
    /// `GITHUB_SHA`, set in every GitHub Actions job, of whatever repository runs it.
    pub github_sha: Option<&'a str>,
    /// `GITHUB_REPOSITORY`, the `owner/name` of the repository the job runs for.
    pub github_repository: Option<&'a str>,
    /// `git rev-parse --show-toplevel`, canonicalised: the work tree git sees from the
    /// crate's directory, which is another repository's if this source is vendored.
    pub git_toplevel: Option<&'a Path>,
    /// This crate's workspace root (`CARGO_MANIFEST_DIR/../..`), canonicalised.
    pub workspace_root: Option<&'a Path>,
    /// Whether the workspace root holds `crates/scarletbook-ffi`.
    pub root_is_ours: bool,
    /// `git rev-parse HEAD`.
    pub git_head: Option<&'a str>,
}

/// The commit to report, or "unknown": `SCARLETBOOK_COMMIT`; else `GITHUB_SHA` when the
/// job runs for this repository; else git's HEAD when git's work tree is this crate's own
/// workspace. Only a hexadecimal object name of 7 to 64 digits is accepted.
pub fn commit(found: &Found<'_>) -> String {
    let ours_ci = found.github_repository == Some(REPOSITORY);
    let ours_git = found.root_is_ours
        && found.workspace_root.is_some()
        && found.git_toplevel.is_some()
        && found.git_toplevel == found.workspace_root;
    [found.explicit, found.github_sha.filter(|_| ours_ci), found.git_head.filter(|_| ours_git)]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|c| is_object_name(c))
        .map_or_else(|| "unknown".to_owned(), str::to_owned)
}

/// Whether `c` is a git object name, full or abbreviated.
fn is_object_name(c: &str) -> bool {
    (7..=64).contains(&c.len()) && c.bytes().all(|b| b.is_ascii_hexdigit())
}
