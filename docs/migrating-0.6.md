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

#### The cursor twins of `Disc` become `audio_cursor` and `frames_cursor` (#305)

Deprecated in 0.6.0 and removed before 1.0. The old names still work and give the same frames,
errors and log lines; each call warns. `Disc::{track_audio, range_audio, track_frames, range_frames,
stored_frame_list, stored_frame_list_cursor, read_track_audio, read_frames}`, `into_cursor()`,
`Disc::continue_*` and `Disc::{track,range}_continuation` stay. `Disc::area_frames` stays as the
diagnostic form of `stored_frame_list`.

`audio_cursor` and `frames_cursor` take `&self`. With `&mut disc`, the iterator's `into_cursor()`
gives the same cursor.

Before:

```rust
let a = disc.track_audio_cursor(slot, n)?;
let b = disc.range_audio_cursor(slot, first, end)?;
let c = disc.track_frames_cursor(slot, n)?;
let d = disc.range_frames_cursor(slot, first, end)?;
```

After:

```rust
use scarletbook::{CursorOptions, FrameSpan};
let a = disc.audio_cursor(slot, FrameSpan::Track(n), CursorOptions::new())?;
let b = disc.audio_cursor(slot, FrameSpan::Range(first, end), CursorOptions::new())?;
let c = disc.frames_cursor(slot, FrameSpan::Track(n), CursorOptions::new())?;
let d = disc.frames_cursor(slot, FrameSpan::Range(first, end), CursorOptions::new())?;
// or, with `&mut disc`:
let a = disc.track_audio(slot, n)?.into_cursor();
```

#### The caller's index moves to `CursorOptions` (#305)

Deprecated in 0.6.0 and removed before 1.0.

Before:

```rust
let a = disc.range_audio_cursor_indexed(slot, first, end, &index)?;
let f = disc.range_frames_cursor_indexed(slot, first, end, &index)?;
```

After:

```rust
let span = FrameSpan::Range(first, end);
let a = disc.audio_cursor(slot, span, CursorOptions::new().with_index(&index))?;
let f = disc.frames_cursor(slot, span, CursorOptions::new().with_index(&index))?;
```

#### `for_range`, `for_range_indexed` and `for_range_speculative` become `open_range` (#305)

Deprecated in 0.6.0 and removed before 1.0, on `TrackCursor` and `TrackAudioCursor` alike. For
cursors without a `Disc`. The defaults are no index and `SeekPolicy::Exact`.

Before:

```rust
let a = TrackCursor::for_range(&area, first, end, n)?;
let b = TrackCursor::for_range_indexed(&area, first, end, n, &index)?;
let c = TrackCursor::for_range_speculative(&area, first, end, n, Some(&index), options)?;
```

After. For an index of `None`, leave out `with_index`.

```rust
use scarletbook::{CursorOptions, SeekPolicy};
let a = TrackCursor::open_range(&area, first, end, n, CursorOptions::new())?;
let b = TrackCursor::open_range(&area, first, end, n, CursorOptions::new().with_index(&index))?;
let with = CursorOptions::new().with_index(&index).with_policy(SeekPolicy::Speculative(options));
let c = TrackCursor::open_range(&area, first, end, n, with)?;
```

#### `continue_to` and `continue_to_range` keep working, for callers without a `Disc` (#305)

Not deprecated, so there is nothing to migrate. `TrackCursor::new`, `TrackAudioCursor::new` and
`AreaCursor::new` stay as well. With a `Disc`, `Disc::continue_*` is the usual form. It isn't the
same call: it uses the disc's seek index and policy, logs each opening, and tells two images of the
same size apart, while `continue_to` always opens exact, with no index and no log.

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
