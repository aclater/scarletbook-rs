<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Releases and verification

[![SLSA Build L2](https://slsa.dev/images/gh-badge-level2.svg)](#verifying-a-download)

Each release is a tag `vX.Y.Z` with a GitHub release that carries the C library, the benchmark
program and the benchmark kit. The crates are published to crates.io by the same release
workflow, with crates.io trusted publishing, only after all of its checks have passed
([RELEASING.md](../RELEASING.md)); a crate whose version is already on crates.io
(`scarletbook-pcm` at 0.7.0 from 1.0.0) is not published again. What changed in each release
is in [CHANGELOG.md](../CHANGELOG.md).

## Release assets

Each release carries the C library as `scarletbook-ffi-<version>-<target>.tar.gz`, with a
SHA-256 file `scarletbook-ffi-<version>-<target>.tar.gz.sha256`. Every one is built and
tested on its platform (`.github/workflows/clib.yml`): a C program linked statically and
dynamically, and Python's ctypes, decode a corpus image and compare TOC and audio with the
conformance manifest.

| `<target>` | Runs on | Built in |
|---|---|---|
| `x86_64-unknown-linux-gnu` | glibc 2.17 or later (manylinux2014) | manylinux2014 image |
| `aarch64-unknown-linux-gnu` | glibc 2.17 or later (manylinux2014) | manylinux2014 image |
| `x86_64-unknown-linux-musl` | musl 1.2 or later (musllinux_1_2), with `libgcc_s.so.1` | musllinux_1_2 image |
| `aarch64-unknown-linux-musl` | musl 1.2 or later (musllinux_1_2), with `libgcc_s.so.1` | musllinux_1_2 image |
| `x86_64-apple-darwin` | macOS 10.12 or later | `MACOSX_DEPLOYMENT_TARGET=10.12` |
| `aarch64-apple-darwin` | macOS 11.0 or later | `MACOSX_DEPLOYMENT_TARGET=11.0` |
| `x86_64-pc-windows-gnu` | Windows 10 or later; links with a UCRT MinGW-w64 GCC (MSYS2 UCRT64 or equivalent) | MinGW-w64 target |
| `x86_64-unknown-linux-gnu-v3` | as `x86_64-unknown-linux-gnu`, on an x86-64-v3 CPU only | manylinux2014 image, `-C target-cpu=x86-64-v3` |
| `x86_64-unknown-linux-musl-v3` | as `x86_64-unknown-linux-musl`, on an x86-64-v3 CPU only | musllinux_1_2 image, `-C target-cpu=x86-64-v3` |

The two `-v3` archives are the x86_64 Linux libraries built a second time for
microarchitecture level x86-64-v3 of the x86-64 psABI: the v3 archive is compiled entirely
for x86-64-v3, every crate of it. They have the same ABI, header and packaging as the
baseline archives next to them, which stay built for the baseline x86-64 level; only the
instructions differ. They are meant for benchmarks and for consumers who build per CPU
(#146).
**They need x86-64-v3: AVX, AVX2, BMI1, BMI2, F16C, FMA, LZCNT, MOVBE and OSXSAVE, on
top of the x86-64-v2 features (CMPXCHG16B, LAHF/SAHF, POPCNT, SSE3, SSE4.1, SSE4.2,
SSSE3).** On a CPU without one of them the library fails with an illegal instruction
(SIGILL), possibly not when it is loaded but on the first call that uses it, so check before
you load it, never after: a program that picks an archive at run time loads the `-v3` one
only if the CPU reports every feature of the level and the operating system has enabled
the AVX state (CPUID's OSXSAVE bit and XGETBV's XMM and YMM bits, or every one of `avx
avx2 bmi1 bmi2 f16c fma abm movbe xsave cx16 lahf_lm popcnt sse4_1 sse4_2 ssse3` in Linux's
`/proc/cpuinfo` flags, where LZCNT is `abm`), and the baseline archive otherwise. glibc 2.33
and later report the level itself: `ld.so --help` lists `x86-64-v3 (supported, searched)`.
`sb_build_info()` tells which one is loaded: the `-v3` library's `target-features` include
`avx2`, `bmi2`, `fma`, `lzcnt` and `movbe` and the rest of the level, the baseline one's are
`fxsr,sse,sse2`. In both, `target` is the triple without `-v3`.

The glibc builds are linked against glibc 2.17 itself, inside the manylinux2014 image, and
the build fails if the shared library asks for a newer glibc symbol version. The
macOS minimums are Rust's own for those targets. The Windows build uses Rust's MinGW-w64
target because its static library (`libscarletbook_ffi.a`) and import library
(`libscarletbook_ffi.dll.a`) are in the format MinGW-w64 GCC links; the DLL exports every
`sb_*` function (checked in CI) and depends only on Windows system DLLs. It uses the
Universal C Runtime (UCRT, `api-ms-win-crt-*.dll`, part of Windows 10 and later), so link
the static library with a UCRT MinGW-w64 toolchain (MSYS2 UCRT64 or equivalent); there is
no msvcrt variant.

Runtime dependencies on Linux: the shared libraries need `libgcc_s.so.1` (the unwinder) as
well as the C library. glibc systems have it as part of GCC's runtime (manylinux allows
it); on Alpine and other musl systems it is the `libgcc` package (`apk add libgcc`). The
static libraries list it in `native-static-libs.txt` (`-lgcc_s`).

Intel CET on x86-64 Linux: the shared library and every object in the static library, of
the `-v3` archives too, carry the shadow stack (SHSTK) property (`readelf -n` shows
`x86 feature: SHSTK`; checked in CI), so a program built with `-fcf-protection` stays SHSTK-marked when it links either one, and
glibc with shadow stacks enabled loads it. The libraries carry no unwinder of their own: a
panic caught at the C boundary unwinds through the system `libgcc_s`, which keeps the shadow
stack in step on distributions that enable it (for example Fedora 44's libgcc 16.2.1 is
itself marked IBT and SHSTK). Shadow stacks are enabled per thread by glibc; musl has no
shadow-stack support today, so on musl systems the mark only matters if the library ends up
in a glibc program. They are not marked for indirect branch tracking
(IBT): stable Rust's precompiled standard library has no `ENDBR64` instructions, so a
program that links the static library loses its IBT mark. Do not force it back with
`-Wl,-z,ibt`. Linux does not enforce IBT in user space today. IBT marking waits for a
stable `cf-protection` option in rustc and a standard library built with it (issue #125).

The PyPA build images are pinned by digest; `.github/workflows/clib.yml` says how to move
to newer ones.

An archive unpacks to the following. This is the layout from 0.6.0 on, whose shared libraries
are named after the C ABI (see [Linking](c-abi.md#linking)). An archive of 0.5.9 or earlier holds a single file
`lib/libscarletbook_ffi.so` (SONAME `libscarletbook_ffi.so`) on Linux and
`lib/libscarletbook_ffi.dylib` (install name `@rpath/libscarletbook_ffi.dylib`) on macOS, with no
versioned name and no link.

```
scarletbook-ffi-<version>-<target>/   (scarletbook-ffi-<version>-<target>-v3/ for a -v3 archive)
  include/scarletbook.h
  lib/libscarletbook_ffi.a           static library (every target)
  lib/libscarletbook_ffi.so.2        shared library (Linux, 0.6.0 on) ...
  lib/libscarletbook_ffi.so          ... and a link to it, for the linker
  lib/libscarletbook_ffi.2.dylib     shared library (macOS, 0.6.0 on) ...
  lib/libscarletbook_ffi.dylib       ... and a link to it, for the linker
  lib/scarletbook_ffi.dll            shared library (Windows) ...
  lib/libscarletbook_ffi.dll.a       ... and its import library
  native-static-libs.txt             system libraries to link after the static library
  README.txt, LICENSE-MIT, LICENSE-APACHE
```

## Verifying a download

```sh
sha256sum -c scarletbook-ffi-<version>-<target>.tar.gz.sha256      # macOS: shasum -a 256 -c
gh attestation verify scarletbook-ffi-<version>-<target>.tar.gz -R aclater/scarletbook-rs \
  --signer-workflow aclater/scarletbook-rs/.github/workflows/release.yml \
  --source-ref refs/tags/v<version> --deny-self-hosted-runners
```

The checksum detects a damaged download, not a replaced one: it is neither signed nor
attested, so whoever could replace the archive could replace its `.sha256` too. The
attestation (a signed SLSA build provenance statement naming the workflow run and commit
that built the file) is what shows the file came from this project's release. With
`--signer-workflow` and `--source-ref`, `gh` accepts only a file built by the release
workflow from the tag `v<version>`; without them, a file attested by any workflow run in the
repository would pass. Releases from 0.2.0 carry the attestation.

## Benchmark assets

Pushing a version tag (`v0.1.0`, matching the crate version) runs the release workflow:
it publishes `bench-<version>-x86_64-linux.tar.gz` (generic x86-64, cargo profile `dist`),
after the correctness gate has passed on that very build, and two labelled builds,
`bench-<version>-x86-64-v3-linux.tar.gz` (`-C target-cpu=x86-64-v3`, not a benchmark asset)
and `bench-<version>-x86_64-linux-znver5.tar.gz` (AMD Zen 5), each with a SHA-256 checksum and, from 0.2.0, a signed
build provenance attestation:

```sh
gh attestation verify bench-<version>-x86_64-linux.tar.gz -R aclater/scarletbook-rs \
  --signer-workflow aclater/scarletbook-rs/.github/workflows/release.yml \
  --source-ref refs/tags/v<version> --deny-self-hosted-runners
```

The checksum detects a damaged download, not a replaced one; the attestation, checked with
both flags, shows that the release workflow built the file from the tag (see [Verifying a
download](#verifying-a-download)).

The release also publishes a benchmark kit for consumers who time scarletbook on their own
machines (#224, #226): `scarletbook-benchkit-<version>-x86_64-linux.tar.gz` and its
`-v3` twin, with statically linked `bench`, `scarletbook-synth` and the scarletbook-pcm
`callpattern` example, a run list of the cases and a reference runner. What a runner
executes and what it sends back: [`benchmarks/README.md`](../benchmarks/README.md).
