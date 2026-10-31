// SPDX-License-Identifier: MIT OR Apache-2.0

//! Records how the library is built, for `sb_build_info` (issue #108): the target, the
//! profile, the enabled features, the target CPU features, the compiler and the commit.
//! Nothing here fails the build: a value that cannot be found is "unknown".

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "build/commit.rs"]
mod commit;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=build/commit.rs");
    println!("cargo::rerun-if-env-changed=GITHUB_SHA");
    println!("cargo::rerun-if-env-changed=GITHUB_REPOSITORY");
    println!("cargo::rerun-if-env-changed=SCARLETBOOK_COMMIT");
    let var = |name: &str| env::var(name).ok().filter(|v| !v.is_empty());

    let target = var("TARGET").unwrap_or_else(|| "unknown".into());
    // Cargo gives build scripts only "debug" or "release"; the profile's own name (for
    // example `dist`, which inherits from release) is the directory of OUT_DIR,
    // <target dir>/[<triple>/]<profile>/build/<package>/out.
    let profile = var("OUT_DIR")
        .and_then(|out| profile_dir(Path::new(&out)))
        .map(|dir| if dir == "debug" { "dev".to_owned() } else { dir })
        .or_else(|| var("PROFILE"))
        .unwrap_or_else(|| "unknown".into());
    let opt_level = var("OPT_LEVEL").unwrap_or_else(|| "unknown".into());
    let mut features: Vec<String> = env::vars()
        .filter_map(|(k, _)| k.strip_prefix("CARGO_FEATURE_").map(|f| f.to_lowercase().replace('_', "-")))
        .collect();
    features.sort();
    let features = if features.is_empty() { "none".to_owned() } else { features.join(",") };
    let mut cpu: Vec<String> = var("CARGO_CFG_TARGET_FEATURE")
        .map(|f| f.split(',').map(str::to_owned).collect())
        .unwrap_or_default();
    cpu.sort();
    let cpu = if cpu.is_empty() { "none".to_owned() } else { cpu.join(",") };
    let rustc = var("RUSTC")
        .and_then(|rustc| Command::new(rustc).arg("--version").output().ok())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|v| v.split_whitespace().nth(1).map(str::to_owned))
        .unwrap_or_else(|| "unknown".into());
    let (explicit, github_sha, github_repository) =
        (var("SCARLETBOOK_COMMIT"), var("GITHUB_SHA"), var("GITHUB_REPOSITORY"));
    let workspace_root =
        var("CARGO_MANIFEST_DIR").and_then(|dir| Path::new(&dir).join("../..").canonicalize().ok());
    let root_is_ours = workspace_root.as_ref().is_some_and(|r| r.join("crates/scarletbook-ffi").is_dir());
    // Git only when no source that would be used first is set: a build without git needs
    // none.
    let ours_ci = github_sha.is_some() && github_repository.as_deref() == Some(commit::REPOSITORY);
    let (git_toplevel, git_head) = if explicit.is_none() && !ours_ci { git_state() } else { (None, None) };
    let commit = commit::commit(&commit::Found {
        explicit: explicit.as_deref(),
        github_sha: github_sha.as_deref(),
        github_repository: github_repository.as_deref(),
        git_toplevel: git_toplevel.as_deref(),
        workspace_root: workspace_root.as_deref(),
        root_is_ours,
        git_head: git_head.as_deref(),
    });

    let info = format!(
        "version={} target={target} profile={profile} opt-level={opt_level} features={features} \
         target-features={cpu} rustc={rustc} commit={commit}",
        var("CARGO_PKG_VERSION").unwrap_or_else(|| "unknown".into())
    );
    // A value with a space or a NUL would break the format: none can have one, but check.
    let info = if info.split(' ').all(|kv| kv.split_once('=').is_some_and(|(_, v)| is_value(v))) {
        info
    } else {
        "version=unknown".to_owned()
    };
    println!("cargo::rustc-env=SB_BUILD_INFO={info}");
}

/// The profile directory of an OUT_DIR, `<profile>/build/<package>/out`.
fn profile_dir(out: &Path) -> Option<String> {
    let build = out.parent()?.parent()?;
    if build.file_name()? != "build" {
        return None;
    }
    build.parent()?.file_name()?.to_str().map(str::to_owned)
}

/// Whether `v` can be a value of the string: printable ASCII without spaces.
fn is_value(v: &str) -> bool {
    !v.is_empty() && v.bytes().all(|b| b.is_ascii_graphic())
}

/// Git's work tree, canonicalised, and HEAD, as seen from this crate's directory, if git
/// is installed and the directory is in a work tree; the build script then reruns when
/// HEAD moves. `commit::commit` decides whether that work tree is this repository's.
fn git_state() -> (Option<PathBuf>, Option<String>) {
    let git = |args: &[&str]| {
        let o = Command::new("git").args(args).output().ok()?;
        o.status.success().then(|| String::from_utf8(o.stdout).ok()).flatten().map(|s| s.trim().to_owned())
    };
    let Some(toplevel) =
        git(&["rev-parse", "--show-toplevel"]).and_then(|t| Path::new(&t).canonicalize().ok())
    else {
        return (None, None);
    };
    for path in [
        git(&["rev-parse", "--git-path", "HEAD"]),
        git(&["rev-parse", "--symbolic-full-name", "HEAD"])
            .and_then(|r| git(&["rev-parse", "--git-path", &r])),
    ]
    .into_iter()
    .flatten()
    {
        if Path::new(&path).exists() {
            println!("cargo::rerun-if-changed={path}");
        }
    }
    (Some(toplevel), git(&["rev-parse", "HEAD"]))
}
