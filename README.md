# scarletbook

[![CI](https://github.com/aclater/scarletbook-rs/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/aclater/scarletbook-rs/actions/workflows/ci.yml)
[![Coverage](https://codecov.io/gh/aclater/scarletbook-rs/graph/badge.svg)](https://codecov.io/gh/aclater/scarletbook-rs)
[![CodeQL](https://github.com/aclater/scarletbook-rs/actions/workflows/codeql.yml/badge.svg?branch=main)](https://github.com/aclater/scarletbook-rs/actions/workflows/codeql.yml)
[![Licence: MIT OR Apache-2.0](https://img.shields.io/badge/licence-MIT%20OR%20Apache--2.0-blue.svg)](#licence-and-credits)
[![MSRV 1.85](https://img.shields.io/badge/MSRV-1.85-blue.svg)](docs/stability.md#minimum-supported-rust-version)
[![OpenSSF Scorecard](https://api.scorecard.dev/projects/github.com/aclater/scarletbook-rs/badge)](https://scorecard.dev/viewer/?uri=github.com/aclater/scarletbook-rs)
[![crates.io](https://img.shields.io/crates/v/scarletbook.svg)](https://crates.io/crates/scarletbook)
[![docs.rs](https://img.shields.io/docsrs/scarletbook)](https://docs.rs/scarletbook)

scarletbook reads Super Audio CD (Scarletbook) disc images, from Rust or from C: the TOC and
its text, the audio areas and tracks, DST decoding, and DSF or DSDIFF files laid out from them.
It is for players, rippers, filesystems and taggers that need a clean-room, permissively
licensed reader, written only from the specifications and conformance corpus of
[aclater/sacd-spec](https://github.com/aclater/sacd-spec).

## Platforms

| System | How | Guide |
|---|---|---|
| Linux x86_64, glibc 2.17 or later | Rust: the crates. C: the `x86_64-unknown-linux-gnu` archive, and `-v3` for x86-64-v3 CPUs. Rust tests and the archive's tests run in CI. | [Rust](docs/rust.md), [C](docs/c-abi.md), [assets](docs/releases.md#release-assets) |
| Linux x86_64, musl 1.2 or later | Rust: the crates. C: the `x86_64-unknown-linux-musl` archive, and `-v3`. The archive's tests run in CI. | [Rust](docs/rust.md), [C](docs/c-abi.md), [assets](docs/releases.md#release-assets) |
| Linux aarch64, glibc 2.17 or musl 1.2 or later | Rust: the crates. C: the `aarch64-unknown-linux-gnu` and `-musl` archives, built and tested on Arm runners in CI. | [Rust](docs/rust.md), [C](docs/c-abi.md), [assets](docs/releases.md#release-assets) |
| macOS 11.0 or later (Apple silicon), 10.12 or later (Intel) | Rust: the crates, tested in CI on Apple silicon. C: the `aarch64-apple-darwin` and `x86_64-apple-darwin` archives, each tested on its CPU in CI. | [Rust](docs/rust.md), [C](docs/c-abi.md), [assets](docs/releases.md#release-assets) |
| Windows 10 or later, x86_64 | Rust: the crates, tested in CI. C: the `x86_64-pc-windows-gnu` archive, for a UCRT MinGW-w64 GCC; tested in CI. | [Rust](docs/rust.md), [C](docs/c-abi.md), [assets](docs/releases.md#release-assets) |
| 32-bit x86 (i686) | Rust only: the crates, built from source; no C archive. CI runs the core, PCM and corpus tests on `i686-unknown-linux-musl` (Pentium 4 and Atom baselines). | [Rust](docs/rust.md) |

Performance figures are measured on an AMD Ryzen 9 3950X (Zen 2); see
[Performance and benchmarks](docs/performance.md).

## Quick start

Both examples read a small synthetic image from this repository's conformance corpus,
[`full-text.iso`](https://github.com/aclater/scarletbook-rs/blob/v1.1.1/corpus/images/full-text.iso)
(1.1 MB, CC0-1.0, [corpus/LICENSE](corpus/LICENSE)): a stereo area with three tracks of one or two
frames each, and every text field filled in. It holds generated test signals, no real audio.
Every command below was run against v1.1.1 on 2026-11-25.

### Rust

```sh
cargo new quickstart && cd quickstart
cargo add scarletbook@=1.1.1
curl -fsSLO https://raw.githubusercontent.com/aclater/scarletbook-rs/v1.1.1/corpus/images/full-text.iso
cat > src/main.rs <<'EOF'
use std::{fs::File, io::BufReader};

use scarletbook::Disc;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let disc = Disc::open(BufReader::new(File::open("full-text.iso")?))?;
    let toc = disc.toc();
    let album = toc.master.text.album_title.as_deref().unwrap_or("(untitled)");
    println!("album: {album:?}");
    for area in &toc.areas {
        println!("{:?} area: {} channels, {} tracks", area.slot, area.channel_count, area.tracks.len());
        for track in &area.tracks {
            let title = track.text.title.as_deref().unwrap_or("(untitled)");
            println!("  track {}: {} frames, {title:?}", track.number, track.duration_frames);
        }
    }
    Ok(())
}
EOF
cargo run
```

```
album: "Album title"
TwoChannel area: 2 channels, 3 tracks
  track 1: 1 frames, "Title 1"
  track 2: 1 frames, "Title 2"
  track 3: 2 frames, "Track 3 title, second item"
```

The `=1.1.1` pin keeps the example on the release whose output is shown. For a project of your
own, [Installing](docs/rust.md#installing) gives a plain `cargo add scarletbook`, and pins only
`scarletbook-pcm` exactly. A frame is 1/75 s.
`{:?}` quotes a string and escapes control characters, including U+007F and U+0080 to U+009F,
while keeping other text (Japanese, say) readable, which `escape_default()` would turn into
`\u{..}` escapes. For audio, see [Using the library](docs/rust.md#using-the-library).

### C

On x86_64 Linux with glibc 2.17 or later and a C compiler. The archive's name and the checks are
those of [Release assets](docs/releases.md#release-assets) and [Verifying a download](docs/releases.md#verifying-a-download).

```sh
mkdir quickstart-c && cd quickstart-c
gh release download v1.1.1 -R aclater/scarletbook-rs -p 'scarletbook-ffi-1.1.1-x86_64-unknown-linux-gnu.tar.gz*'
sha256sum -c scarletbook-ffi-1.1.1-x86_64-unknown-linux-gnu.tar.gz.sha256
gh attestation verify scarletbook-ffi-1.1.1-x86_64-unknown-linux-gnu.tar.gz -R aclater/scarletbook-rs \
  --signer-workflow aclater/scarletbook-rs/.github/workflows/release.yml --source-ref refs/tags/v1.1.1 \
  --deny-self-hosted-runners
tar xzf scarletbook-ffi-1.1.1-x86_64-unknown-linux-gnu.tar.gz
curl -fsSLO https://raw.githubusercontent.com/aclater/scarletbook-rs/v1.1.1/corpus/images/full-text.iso
cat > toc.c <<'EOF'
#include <stdio.h>
#include "scarletbook.h"

/* Prints len bytes, escaping every byte outside 0x20 to 0x7E (a UTF-8 letter becomes \xNN bytes). */
static void put(const char *s, size_t len) {
    for (size_t i = 0; i < len; i++) {
        unsigned char c = (unsigned char)s[i];
        if (c >= 0x20 && c <= 0x7E) putchar(c); else printf("\\x%02X", c);
    }
}

int main(void) {
    struct SbDisc *disc = NULL;
    struct SbDiscInfo info;
    char title[256];
    size_t len = 0;

    if (sb_abi_version() != SB_ABI_VERSION) return 1;  /* header and library must agree */
    if (sb_disc_open_file("full-text.iso", &disc) != SB_STATUS_OK) {
        fprintf(stderr, "%s\n", sb_last_error_message());
        return 1;
    }
    sb_disc_info(disc, &info);
    if (sb_disc_string(disc, SB_DISC_STRING_ALBUM_TITLE, title, sizeof title, &len) == SB_STATUS_OK)
        { printf("ABI %u, album: ", sb_abi_version()); put(title, len); putchar('\n'); }
    for (uint32_t a = 0; a < info.area_count; a++) {
        struct SbAreaInfo area;
        sb_disc_area(disc, a, &area);
        printf("area slot %u: %u channels, %u tracks\n", area.slot, area.channel_count, area.track_count);
        for (uint32_t t = 1; t <= area.track_count; t++) {
            struct SbTrackInfo track;
            sb_track_info(disc, area.slot, t, &track);
            printf("  track %u: %u frames, ", t, track.duration_frames);
            if (sb_track_string(disc, area.slot, t, SB_TRACK_STRING_TITLE, title, sizeof title, &len) == SB_STATUS_OK)
                put(title, len);
            putchar('\n');
        }
    }
    sb_disc_free(disc);
    return 0;
}
EOF
P=scarletbook-ffi-1.1.1-x86_64-unknown-linux-gnu
cc -std=c99 -Wall -Wextra -pedantic -I$P/include toc.c -L$P/lib -lscarletbook_ffi -Wl,-rpath,'$ORIGIN/'$P/lib -o toc
./toc
```

```
scarletbook-ffi-1.1.1-x86_64-unknown-linux-gnu.tar.gz: OK
ABI 2, album: Album title
area slot 0: 2 channels, 3 tracks
  track 1: 1 frames, Title 1
  track 2: 1 frames, Title 2
  track 3: 2 frames, Track 3 title, second item
```

Titles and other text come from the disc and can hold control characters (U+007F, U+0080 to U+009F,
and U+009B starts a control sequence on some terminals), so both examples escape what they print;
code of yours should do the same, as the docs of
[`DiscText`](https://docs.rs/scarletbook/1.1.1/scarletbook/toc/struct.DiscText.html) and of
[`sb_disc_string`](crates/scarletbook-ffi/include/scarletbook.h) say.

`gh attestation verify` exits with status 0 when it succeeds, and non-zero on a failure. In a
terminal it also prints the policy it checked and "Verification succeeded!"; when its output is
not a terminal, as in a script, it prints nothing on success.
The program records `libscarletbook_ffi.so.2`, the library's name for C ABI 2, and finds it through
the rpath, so it runs from this directory without the link. To ship it, copy
`lib/libscarletbook_ffi.so.2` next to the program, as [Linking](docs/c-abi.md#linking) says. See the [C ABI guide](docs/c-abi.md)
for the rest of the API, and [Migrating to 0.7.0](docs/migrating-0.7.md) and
[Migrating to 0.6.0](docs/migrating-0.6.md) from an earlier release.

## Documentation

The [documentation index](docs/README.md) groups every guide by task.

- [Rust guide](docs/rust.md): installing, reading audio, stored frames, DSF and DSDIFF files,
  seek indexes and provisional reading, features
- [C ABI guide](docs/c-abi.md): the C library, [Linking](docs/c-abi.md#linking) and
  [ABI compatibility](docs/c-abi.md#abi-compatibility), the rules for `SB_ABI_VERSION`
- [Using from Python (ctypes)](docs/python.md): bindings for every struct, and
  [the whole TOC in one call](docs/python.md#the-whole-toc-in-one-call)
- [Releases and verification](docs/releases.md): the release assets, checksums, attestations
  and crates.io publishing
- [Stability and versions](docs/stability.md): the 1.0 promise, versions, the minimum
  supported Rust version, and `scarletbook-pcm` (decommissioned
  ([#398](https://github.com/aclater/scarletbook-rs/issues/398)): frozen at 0.7.0 on
  crates.io, no further releases; consumers convert DSD to PCM in their own crate instead)
- [Performance and benchmarks](docs/performance.md): the benchmark program, the benchmark
  kit, and measuring performance
- [Contributing and the clean room](docs/contributing.md): the repository layout, the
  clean-room rule ([CONTRIBUTING.md](CONTRIBUTING.md), [CLEANROOM.md](CLEANROOM.md)), and
  changing the C ABI
- [Migrating to 0.7.0](docs/migrating-0.7.md) and [Migrating to 0.6.0](docs/migrating-0.6.md)
- [CHANGELOG.md](CHANGELOG.md), [RELEASING.md](RELEASING.md) and [SECURITY.md](SECURITY.md)
  (reporting vulnerabilities)

## Licence and credits

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.

Written only from the specifications and conformance corpus of
[aclater/sacd-spec](https://github.com/aclater/sacd-spec); the corpus in `corpus/` is copied
from it unchanged (CC0-1.0, [corpus/LICENSE](corpus/LICENSE)).
