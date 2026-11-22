# Migrating to 0.7.0

0.7.0 removes the 36 items that 0.6.0 deprecated, as [Migrating to 0.6.0](migrating-0.6.md)
announced: "deprecated in 0.6.0 and removed before 1.0"
([#39](https://github.com/aclater/scarletbook-rs/issues/39), item R1). Nothing else in
the interface changes, and neither does any behaviour: each removed item was another name or
another call for one that stays, and gave the same values, frames, errors and log lines.

Code that builds on 0.6 without deprecation warnings builds on 0.7.0 unchanged. Code that still
uses one of these items gets a compile error naming it; this page gives the replacement for each.
The 0.6.0 page has the longer notes of the 0.6.0 changes, under the
section given in each heading below.

The C library does not change: no C function or constant was deprecated, the header is the same
and `SB_ABI_VERSION` stays 2.

## Rust API

### The cursor twins of `Disc` (M6)

Removed: `Disc::{track_audio_cursor, range_audio_cursor, range_audio_cursor_indexed,
track_frames_cursor, range_frames_cursor, range_frames_cursor_indexed}`. Use
`Disc::audio_cursor` and `Disc::frames_cursor` with a `FrameSpan` and `CursorOptions`, or, with
`&mut disc`, the borrowing iterator and `into_cursor()`.

Before:

```rust
let a = disc.track_audio_cursor(slot, n)?;
let b = disc.range_audio_cursor(slot, first, end)?;
let c = disc.track_frames_cursor(slot, n)?;
let d = disc.range_frames_cursor(slot, first, end)?;
let e = disc.range_audio_cursor_indexed(slot, first, end, &index)?;
let f = disc.range_frames_cursor_indexed(slot, first, end, &index)?;
```

After:

```rust
use scarletbook::{CursorOptions, FrameSpan};
let a = disc.audio_cursor(slot, FrameSpan::Track(n), CursorOptions::new())?;
let b = disc.audio_cursor(slot, FrameSpan::Range(first, end), CursorOptions::new())?;
let c = disc.frames_cursor(slot, FrameSpan::Track(n), CursorOptions::new())?;
let d = disc.frames_cursor(slot, FrameSpan::Range(first, end), CursorOptions::new())?;
let e = disc.audio_cursor(slot, FrameSpan::Range(first, end), CursorOptions::new().with_index(&index))?;
let f = disc.frames_cursor(slot, FrameSpan::Range(first, end), CursorOptions::new().with_index(&index))?;
// or, with `&mut disc`:
let a = disc.track_audio(slot, n)?.into_cursor();
```

### `for_range`, `for_range_indexed` and `for_range_speculative` (M6)

Removed on `TrackCursor` and `TrackAudioCursor` alike (six methods). Use `open_range` with
`CursorOptions`; its defaults are no index and `SeekPolicy::Exact`.

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

### `speculation_id()` and `counts_from()` (S1)

Removed on `AreaCursor`, `TrackCursor`, `TrackFrames`, `TrackAudioCursor` and `TrackAudio` (ten
methods). Use `claim()` and `claim_lsn()`, which return the same values.

Before:

```rust
let (id, from) = (cursor.speculation_id(), cursor.counts_from());
```

After:

```rust
let (id, from) = (cursor.claim(), cursor.claim_lsn());
```

### The `*with_log` and `*with_format*` constructors (S1)

Removed: `Image::open_with_log`, `Image::with_format`, `Image::with_format_and_log`,
`Disc::open_with_log`, `Disc::open_with_format` and `Disc::open_with_format_and_log`. Use
`Image::options()` and `Disc::options()`. `open`, `from_image`, `share` and `share_with_log` stay.

Before:

```rust
let a = Disc::open_with_log(reader, log)?;
let b = Disc::open_with_format(reader, format)?;
let c = Disc::open_with_format_and_log(reader, format, log)?;
let d = Image::open_with_log(reader, log)?;
let e = Image::with_format(reader, format)?;
let f = Image::with_format_and_log(reader, format, log)?;
```

After:

```rust
let a = Disc::options().with_log(log).open(reader)?;
let b = Disc::options().with_format(format).open(reader)?;
let c = Disc::options().with_format(format).with_log(log).open(reader)?;
let d = Image::options().with_log(log).open(reader)?;
let e = Image::options().with_format(format).open(reader)?;
let f = Image::options().with_format(format).with_log(log).open(reader)?;
```

### `Stored::Plain` (S1)

Removed: the constant `Stored::Plain`. Use the variant `Stored::Dsd`. A path written
`Stored::Plain` now fails to build.

**A pattern `Plain` under `use Stored::*` still builds, and is wrong.** It names no variant, so it
is a new binding that matches every value, and its arm also takes `Dst` and `Missing`; the
compiler gives only warnings (`unreachable_patterns`, `unused_variables`, `non_snake_case`). This
was so in 0.6.0 already and the removal does not change it. To find such patterns:

```sh
grep -rnE 'Stored::\*' --include='*.rs' .
RUSTFLAGS="-D unreachable_patterns -D unused_variables" cargo build --all-targets
```

Before:

```rust
match stored {
    Stored::Dst(bytes) => write(&bytes),
    Stored::Plain => write(&frame.data),
    Stored::Missing => {}
}
```

After:

```rust
match stored {
    Stored::Dst(bytes) => write(&bytes),
    Stored::Dsd => write(&frame.data),
    Stored::Missing => {}
}
```

### `AreaIndex`'s count methods (S2)

Removed: `AreaIndex::{checkpoints, hints, confirmations, pending_claims}`. Use
`checkpoint_count`, `hint_count`, `confirmation_count` and `pending_claim_count`, which return
the same counts.

Before:

```rust
let kept = (index.checkpoints(), index.hints(), index.confirmations(), index.pending_claims());
```

After:

```rust
let kept = (
    index.checkpoint_count(),
    index.hint_count(),
    index.confirmation_count(),
    index.pending_claim_count(),
);
```

### `dst::FL` and `dst::MAX_CHANNELS` (N3)

Removed. Use `audio::FRAME_BYTES_PER_CHANNEL` and `audio::MAX_CHANNELS`, with the same values.
`audio::MAX_CHANNELS` is a `u8`, where `dst::MAX_CHANNELS` was a `usize`, so a size needs a
conversion. `scarletbook_pcm::MAX_CHANNELS` and `scarletbook_pcm::FRAME_BYTES_PER_CHANNEL` stay.

Before:

```rust
let buf = vec![0u8; dst::FL * dst::MAX_CHANNELS];
```

After:

```rust
let buf = vec![0u8; audio::FRAME_BYTES_PER_CHANNEL * usize::from(audio::MAX_CHANNELS)];
```

### `Cutoff::Default` (N5)

Removed from `scarletbook-pcm`: the constant `Cutoff::Default`. Use the variant `Cutoff::Standard`,
the same filter with the same output. `Cutoff::default()` is still this filter, and the C constant
`SB_PCM_CUTOFF_DEFAULT` keeps its name and value.

**A pattern `Default` under `use Cutoff::*` still builds, and is wrong**, for the same reason as
`Plain` above: its arm also takes `Low` and `Wide`. Search for `Cutoff::\*` and build with the
same warnings as errors.

Before:

```rust
let config = PcmConfig::new(PcmRate::Hz176400, Cutoff::Default, 2);
```

After:

```rust
let config = PcmConfig::new(PcmRate::Hz176400, Cutoff::Standard, 2);
```

## C library

Nothing to do: the header and the ABI are unchanged, and `SB_ABI_VERSION` stays 2.
