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

#### Plumbing, tuning values and text helpers are hidden (#297)

Hidden in 0.6.0 with `#[doc(hidden)]`, and out of the semver promise: each may change or go in any
release, with no deprecation first. Calls still compile, and output and the C ABI do not change.
Code that uses only documented items has nothing to do.

**`Continuation`:** `log_audio`, `audio_unlogged`, `audio_as`, `log_frames`, `frames_unlogged`,
`frames_as`, `check_index`, `log_audio_indexed`, `audio_indexed_unlogged`, `audio_indexed_as`,
`log_frames_indexed`, `frames_indexed_unlogged`, `frames_indexed_as`, `log_fresh_frames`,
`fresh_frames`, and the enum `Opening`. `Continuation::{range, audio, frames}` stay.

Before:

```rust
let opening = next.log_audio(&prev);
let cursor = next.audio_as(prev, opening);
```

After:

```rust
let cursor = next.audio(prev);
```

**Tuning:** `dst::lanes`, `dst::MAX_LANES`, `audio::MAX_PARALLEL_BATCH`,
`audio::PARALLEL_FRAMES_PER_THREAD`, `seek::MAX_CLAIMS`, `seek::MAX_EVENTS`, `seek::HINT_REACH` and
`seek::REPLAY_SECTORS`. The constants have no replacement: don't size anything by them.

Before:

```rust
let group = scarletbook::dst::lanes(channels);
for (frames, outs) in frames.chunks(group).zip(outs.chunks_mut(group)) {
    decoder.decode_many(frames, channels, outs);
}
```

After, with the same output. `decode_many` splits the frames into groups itself, so its speed may
differ from grouping by `lanes`; a caller who keeps that loop keeps its speed, without a promise.

```rust
decoder.decode_many(&frames, channels, &mut outs);
```

**Text:** `text::{decode, decode_string, fixed_bytes, fixed_text, language, string_bytes,
text_string}`. `text::Charset` and `text::MAX_STRING_BYTES` stay public.

Before:

```rust
let title = scarletbook::text::text_string(buf, offset, charset);
```

After: the decoded fields of `Toc`, `Area` and `Track` and their text, instead of parsing TOC bytes.

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

Not written yet.

## C library

### M8. The shared library's name

ABI 2 is the 1.0 ABI, so the header and the ABI do not change. The shared library's name will
carry the ABI version on Linux and macOS.

Not written yet.
