# Migrating to 0.6.0

0.6.0 is the release that changes the interface before it is frozen for 1.0: one migration, then
one consumer cycle, then 1.0.0 with no further API change
([#39](https://github.com/aclater/scarletbook-rs/issues/39), owner decision of 2026-11-06). This
page gives a short before-and-after for each change, so that a consumer can move item by item.

**This page is a skeleton until 0.6.0 is released.** A section that still says "Not written yet"
has no change on `main`. Nothing here applies to 0.5.x.

Where it can, 0.6.0 deprecates the old name instead of removing it, and the removal follows
before 1.0. An entry says which of the two it is, so code that builds with deprecation warnings
on 0.6.0 still has that entry to act on.

## How this page is filled

Each pull request that makes one of these changes does both of these itself:

1. Adds its [`changelog.d/`](../changelog.d/README.md) fragment under `Changed`, `Removed` or
   `Deprecated`. The fragment says that the change is breaking and ends with the section of this
   page, for example "Migration: `docs/migrating-0.6.md`, M1".
2. Replaces "Not written yet" in that section with an entry, or adds its entry after the ones
   already there:

````markdown
#### `Old::name` is now `New::name` (#PR)

Removed in 0.6.0, or: deprecated in 0.6.0 and removed before 1.0. Say who is affected, and what
does not change, in a sentence or two.

Before:

```rust
// the shortest code that shows the old use
```

After:

```rust
// the same code on 0.6.0
```
````

Use `c` blocks for changes to the C library. Each section already has its heading, so pull
requests working on different items do not touch the same lines.

Preparing the 0.6.0 release needs every section filled in, so `grep -c '^Not written yet\.$'
docs/migrating-0.6.md` must give 0. The release preparation then removes this part and the
skeleton note above, and links this page from the README.

## Rust API

The audit's item numbers are in
[#39](https://github.com/aclater/scarletbook-rs/issues/39#issuecomment-6556073930).

### M1. TOC structs are `#[non_exhaustive]`

`toc::Toc`, `MasterToc`, `Area`, `Track`, `Locale`, `DiscText`, `AreaText` and `TrackText`.

Not written yet.

### M2. Result structs are `#[non_exhaustive]`

`error::Warning`, `audio::AreaFrame`, `TrackFrame` and `AudioFrame`, and the new
`TrackFrame::new`.

Not written yet.

### M3. Enum variants with fields are `#[non_exhaustive]`

`seek::Speculation`, `seek::ResumedFrom` and the `Error` variants with fields.

Not written yet.

### M4. Enums: non-exhaustive or frozen

`audio::FrameStatus`, `audio::DecodeOn`, `image::SectorFormat`, `toc::LoudspeakerLayout`,
`toc::Speaker` and `toc::TrackTextItem`, and the enums documented as frozen.

Not written yet.

### M5. Plumbing and tuning knobs leave the promise

The FFI-only methods of `audio::Continuation`, the decoder and scheduler tuning constants, and
the low-level `text` helpers.

Not written yet.

### M6. One entry point per concept

The range, track and stored-frame entry points of `Disc`, `TrackAudioCursor` and `TrackCursor`.

Not written yet.

### S1. Names and constructors

The provisional-reading id, the start sector, the `*with_format*` constructors, and `Stored::Plain`.

Not written yet.

### S2. Count methods

`AreaIndex::{checkpoints, hints, confirmations, pending_claims}`.

Not written yet.

### S3. `Image::read_sectors`

Not written yet.

### N3. Duplicate constants

`MAX_CHANNELS`, `dst::FL` and `audio::FRAME_BYTES_PER_CHANNEL`.

Not written yet.

### N5. `Cutoff::Default`

#### `Cutoff::Default` is now `Cutoff::Standard` (#291)

**Check this first: a match can silently take the wrong branch.** Under `use Cutoff::*`, a pattern
`Default` no longer names a filter. It becomes a new binding that matches every filter, so that arm
also takes `Low` and `Wide`. The code still builds, with only warnings (`unreachable_patterns`,
`unused_variables`, `non_snake_case`). To find it, search for the glob, then change each such
pattern to `Standard`:

```sh
grep -rnE 'Cutoff::\*' --include='*.rs' .
```

While migrating, also build with those warnings as errors:
`RUSTFLAGS="-D unreachable_patterns -D unused_variables" cargo build --all-targets`. Removing the
old name before 1.0 won't change this: it comes from the rename, and only a path written
`Cutoff::Default` gives an error then.

Deprecated in 0.6.0 and removed before 1.0: the old name stays as a constant, so expressions and
patterns written `Cutoff::Default` on a `Cutoff` value still build, with a warning. What stops building now is `use
Cutoff::Default` and `Cutoff::Default` in a pattern on a `&Cutoff`, and `{:?}` of the filter prints
`Standard`. The filter and its output bytes do not change, `Cutoff::default()` is still this filter,
and the C constant `SB_PCM_CUTOFF_DEFAULT` keeps its name and value.

Before:

```rust
let config = PcmConfig::new(PcmRate::Hz176400, Cutoff::Default, 2);
```

After:

```rust
let config = PcmConfig::new(PcmRate::Hz176400, Cutoff::Standard, 2);
```

## C library

### M8. The shared library's name

ABI 2 is the 1.0 ABI, so the header and the ABI do not change. The shared library's name will
carry the ABI version on Linux and macOS.

Not written yet.
