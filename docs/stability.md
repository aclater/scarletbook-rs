<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Stability and versions

## Stability

1.0.0 freezes the interface: it is the 0.7 interface, with no API or ABI change
([#39](https://github.com/aclater/scarletbook-rs/issues/39)). From 1.0.0, the Rust API of
`scarletbook` and the C ABI of `scarletbook-ffi` follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html): they stay compatible within a major
version, by the rules of
[Cargo's SemVer reference](https://doc.rust-lang.org/cargo/reference/semver.html), so there is
no breaking change without 2.0.0, and every breaking change is listed in
[CHANGELOG.md](../CHANGELOG.md). A `1.y.z` release may add to the interface (a minor release,
`1.Y.0`) or only fix it (a patch release, `1.y.Z`); each release is checked with
`cargo semver-checks` against the previous one ([RELEASING.md](../RELEASING.md)).

The C library keeps `SB_ABI_VERSION` 2 and the library names `libscarletbook_ffi.so.2` and
`libscarletbook_ffi.2.dylib` throughout 1.x. What counts as a breaking change of the C ABI is
in [ABI compatibility](c-abi.md#abi-compatibility).

`scarletbook-pcm` is outside this promise. It stays at 0.7.0 and is not part of the 1.0
freeze. Its deprecation is planned ([#398](https://github.com/aclater/scarletbook-rs/issues/398)),
as DSD-to-PCM conversion moves to its consumers; it gets no new features. Until then it follows
Cargo's rules for `0.y.z` (the `y` step may break it, the `z` step may only add). The `sb_pcm_*`
functions of the C library are part of ABI 2 and stay in it, even once they are marked
deprecated.

Before 1.0, as in Cargo, the `y` step of `0.y.z` was the major one and could break the API.
Coming from 0.6 or earlier, [Migrating to 0.7.0](migrating-0.7.md) (and, before it,
[Migrating to 0.6.0](migrating-0.6.md)) gives a before-and-after for each change; 0.7 to
1.0.0 needs only the version.

Not covered:

- items marked `#[doc(hidden)]`;
- the `fuzzing` feature of `scarletbook-ffi`, which exists for the fuzz targets;
- the Rust items of `scarletbook-ffi`, whose interface is the C ABI;
- `scarletbook-bench` and `scarletbook-conformance`, which are not published.

## Versions

`scarletbook`, `scarletbook-ffi` and the unpublished crates share the workspace version, so a
breaking change in one raises the version of them all, including crates that did not change
([#39](https://github.com/aclater/scarletbook-rs/issues/39), owner decision of 2026-11-06).
`scarletbook-pcm` is the exception from 1.0.0: it keeps its own version, 0.7.0, outside the
freeze ([#398](https://github.com/aclater/scarletbook-rs/issues/398)), and a release that
leaves it unchanged does not publish it again.

## Minimum supported Rust version

The minimum supported Rust version (MSRV) is 1.85, declared as `rust-version` in
[Cargo.toml](../Cargo.toml).

- **When it can rise:** only in a release that changes the `y` of `x.y.z`, so `0.Y.0` before
  1.0 and `x.Y.0` from 1.0. It never rises in a release that changes only `z`.
- **How far it can rise:** only to a Rust release at least 12 months old, so that every Rust
  release of the last 12 months stays supported. The MSRV does not rise without a reason, such
  as a feature the code needs.
- **How you hear of it:** the rise is listed under "Changed" in CHANGELOG.md.

CI's `msrv` job checks `scarletbook`, `scarletbook-pcm` and `scarletbook-ffi` with all features
on this version, using the repository's `Cargo.lock`. It does not check the lowest dependency
versions that the crates declare, and it does not run the tests on this version.
