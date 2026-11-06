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

#### The TOC structs can't be built by literal or destructured without `..` (#298)

Changed in 0.6.0, with no deprecation first. This affects code outside the crate that uses struct
patterns or struct literals on these types. Reading a field works as before, and the fields stay
public and assignable. Take an `Area` or `Track` from a parsed `Toc`, and make a text struct from
`Default`.

Before:

```rust
let Locale { language, language_code, charset } = locale;
let text = TrackText { title: Some(title), ..TrackText::default() };
```

After:

```rust
let Locale { language, language_code, charset, .. } = locale;
let mut text = TrackText::default();
text.title = Some(title);
```

### M2. Result structs are `#[non_exhaustive]`

`error::Warning`, `audio::AreaFrame`, `TrackFrame` and `AudioFrame`, and the new
`TrackFrame::new`.

#### `TrackFrame` is built with `TrackFrame::new`, and patterns need `..` (#301)

Changed in 0.6.0, with no deprecation first. This affects code outside the crate that uses struct
patterns or struct literals on `Warning`, `AreaFrame`, `TrackFrame` or `AudioFrame`. Reading a field
works as before. Only `TrackFrame` has a constructor, for frames a program saved and hands to
`audio::decode_frame`; the library alone produces the other three.

Before:

```rust
let frame = TrackFrame { number, content: FrameContent::Dst(saved) };
let AudioFrame { number, status, data } = decode_frame(&mut decoder, frame, channels)?;
```

After:

```rust
let frame = TrackFrame::new(number, FrameContent::Dst(saved));
let AudioFrame { number, status, data, .. } = decode_frame(&mut decoder, frame, channels)?;
```

### M3. Enum variants with fields are `#[non_exhaustive]`

`seek::Speculation`, `seek::ResumedFrom` and the `Error` variants with fields.

#### Patterns on twelve variants need `..`, and the variants can't be built (#302)

Changed in 0.6.0, with no deprecation first. The variants are
`Speculation::{Pending, Confirmed, Refuted, Unverified}`,
`ResumedFrom::{Checkpoint, Discarded, Speculative}` and
`Error::{MasterTocVersionUnsupported, ChannelCountUnsupported, TrackExceedsCapacity, NoSuchTrack, FrameRangeInvalid}`.
A pattern needs `..` even when it names every field. A program that made one of these `Error`s
itself needs its own error type or variant for that case.

Before:

```rust
match cursor.speculation() {
    Speculation::Refuted { lsn, first, last_returned, exact_from } => redo(first, exact_from),
    Speculation::Confirmed { lsn, first } => keep(lsn, first),
    _ => {}
}
if let Err(Error::NoSuchTrack { slot, track }) = result { report(slot, track) }
```

After:

```rust
match cursor.speculation() {
    Speculation::Refuted { first, exact_from, .. } => redo(first, exact_from),
    Speculation::Confirmed { lsn, first, .. } => keep(lsn, first),
    _ => {}
}
if let Err(Error::NoSuchTrack { slot, track, .. }) = result { report(slot, track) }
```

### M4. Enums: non-exhaustive or frozen

`audio::FrameStatus`, `audio::DecodeOn`, `image::SectorFormat`, `toc::LoudspeakerLayout`,
`toc::Speaker` and `toc::TrackTextItem`, and the enums documented as frozen.

#### A `match` on six enums needs a wildcard arm (#304)

Changed in 0.6.0, with no deprecation first. This affects `FrameStatus`, `DecodeOn`, `SectorFormat`,
`LoudspeakerLayout`, `Speaker` and `TrackTextItem`, so that later variants aren't breaking changes.
Nothing changes for `Slot`, `AreaKind`, `DiscTextItem`, `AreaTextItem`, `FrameKind`, `FrameContent`,
`Stored` and `Level`: they are documented as frozen, each with its reason, and a `match`
on them needs no wildcard arm. `Version`, `Genre`, `Date`, `Timecode` and `AreaPointer` are
documented as frozen too, so they can still be built and matched without `..`: nothing to migrate.

Before:

```rust
let good = match frame.status {
    FrameStatus::Dsd | FrameStatus::Decoded => true,
    FrameStatus::Missing | FrameStatus::Invalid(_) => false,
};
```

After:

```rust
let good = match frame.status {
    FrameStatus::Dsd | FrameStatus::Decoded => true,
    FrameStatus::Missing | FrameStatus::Invalid(_) => false,
    // A later outcome: decide what it should mean for this program.
    _ => false,
};
```

#### `LoudspeakerLayout::Unspecified` needs `..` and can't be built (#304)

Changed in 0.6.0, with no deprecation first. A pattern needs `..` even when it names both fields.
`LoudspeakerLayout::new` makes the same value.

Before:

```rust
let layout = LoudspeakerLayout::Unspecified { channels: 4, config: 0 };
if let LoudspeakerLayout::Unspecified { channels, config } = layout { report(channels, config) }
```

After:

```rust
let layout = LoudspeakerLayout::new(4, 0);
if let LoudspeakerLayout::Unspecified { channels, config, .. } = layout { report(channels, config) }
```

#### Comparing with a built `Speculation` or `ResumedFrom` needs `matches!` (#302, #304)

Changed in 0.6.0, with no deprecation first. This follows from M3: both types are still `PartialEq`,
but code outside the crate can't build their variants with fields, so `assert_eq!` against one no
longer compiles. `Speculation::Exact`, `ResumedFrom::Start` and `ResumedFrom::End` have no fields
and can still be compared. The same goes for `LoudspeakerLayout::Unspecified`, above.

Before:

```rust
assert_eq!(cursor.speculation(), Speculation::Confirmed { lsn: 16, first: 4 });
assert_eq!(cursor.resumed_from(), ResumedFrom::Discarded { lsn: 16 });
```

After:

```rust
assert!(matches!(cursor.speculation(), Speculation::Confirmed { lsn: 16, first: 4, .. }));
assert!(matches!(cursor.resumed_from(), ResumedFrom::Discarded { lsn: 16, .. }));
```

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

The `*_indexed_*` methods continued a cursor from an index the caller keeps. They have no public
form of their own. Give the index to the disc, or take what the cursor recorded and open a new
cursor from the index (`audio_cursor` is in section M6). The new cursor starts from the index
instead of going on with the earlier scan, and gives the same frames.

Before:

```rust
let next = disc.range_continuation(slot, first, end)?;
next.check_index(&index)?;
let opening = next.log_audio_indexed(&prev, &index);
let cursor = next.audio_indexed_as(prev, &mut index, opening);
```

After, with the disc keeping the index:

```rust
disc.set_seek_index(slot, index)?;
let cursor = disc.continue_range_audio(prev, slot, first, end)?;
```

Or, with the caller keeping it:

```rust
index.absorb(&mut prev)?;
let span = FrameSpan::Range(first, end);
let cursor = disc.audio_cursor(slot, span, CursorOptions::new().with_index(&index))?;
```

**Tuning:** `audio::MAX_PARALLEL_BATCH`, `audio::PARALLEL_FRAMES_PER_THREAD`,
`seek::MAX_CLAIMS`, `seek::MAX_EVENTS`, `seek::HINT_REACH` and `seek::REPLAY_SECTORS`. They have no
replacement: code that reads one should stop sizing anything by it. `dst::lanes` stays public and
documented, as the best number of frames to pass to `DstDecoder::decode_many` at once. The function
is stable, but the number it returns is a tuning value: it may change in any release, and may
differ per build, per CPU and with any setting that affects decoding. `dst::MAX_LANES` also stays
public, as the limit of `TrackAudioCursor::with_dst_lanes`; its value may change in any release too.

**Text:** `text::{decode, decode_string, fixed_bytes, fixed_text, language, string_bytes,
text_string}` and `text::Charset`, which only they take. `Locale.charset` stays a `u8`, and
`text::MAX_STRING_BYTES` stays public.

Before:

```rust
let title = scarletbook::text::text_string(buf, offset, charset);
```

After: the decoded fields of `Toc`, `Area` and `Track` and their text, instead of parsing TOC bytes.

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

#### `speculation_id()` is now `claim()`, and `counts_from()` is now `claim_lsn()` (#300)

Deprecated in 0.6.0 and removed before 1.0, on `AreaCursor`, `TrackCursor`, `TrackFrames`,
`TrackAudioCursor` and `TrackAudio`. They return the same values. Nothing stops building until the
removal, except in builds that deny warnings.

Before:

```rust
let (id, from) = (cursor.speculation_id(), cursor.counts_from());
```

After:

```rust
let (id, from) = (cursor.claim(), cursor.claim_lsn());
```

#### The `*with_log` and `*with_format*` constructors give way to `options()` (#300)

Deprecated in 0.6.0 and removed before 1.0: `Image::open_with_log`, `Image::with_format`,
`Image::with_format_and_log`, `Disc::open_with_log`, `Disc::open_with_format` and
`Disc::open_with_format_and_log`. They behave the same. Nothing stops building until the removal,
except in builds that deny warnings. `open`, `from_image`, `share` and `share_with_log` stay.

Before:

```rust
let a = Disc::open_with_log(reader, log)?;
let b = Disc::open_with_format(reader, format)?;
let c = Disc::open_with_format_and_log(reader, format, log)?;
let d = Image::with_format(reader, format)?;
```

After:

```rust
let a = Disc::options().with_log(log).open(reader)?;
let b = Disc::options().with_format(format).open(reader)?;
let c = Disc::options().with_format(format).with_log(log).open(reader)?;
let d = Image::options().with_format(format).open(reader)?;
```

#### `Stored::Plain` is now `Stored::Dsd` (#300)

**Check this first: a match can silently take the wrong branch.** Under `use Stored::*`, a pattern
`Plain` no longer names a variant. It becomes a new binding that matches every value, so that arm
also takes `Dst` and `Missing`. The code still builds, with only warnings (`unreachable_patterns`,
`unused_variables`, `non_snake_case`). To find it, search for the glob, then change each such
pattern to `Dsd`:

```sh
grep -rnE 'Stored::\*' --include='*.rs' .
```

While migrating, also build with those warnings as errors:
`RUSTFLAGS="-D unreachable_patterns -D unused_variables" cargo build --all-targets`. Removing the
old name before 1.0 won't change this: it comes from the rename, and only a path written
`Stored::Plain` gives an error then.

Deprecated in 0.6.0 and removed before 1.0: the old name stays as a constant, so expressions and
patterns written `Stored::Plain` on a `Stored` value still build, with a warning. What stops building
now is `use Stored::Plain`, and `Stored::Plain` in a pattern through a reference: on a `&Stored`, as
`Some(Stored::Plain)` on an `Option<&Stored>`, or inside a tuple of references. `{:?}` of the value
prints `Dsd`. Its meaning does not change.

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

### S2. Count methods

`AreaIndex::{checkpoints, hints, confirmations, pending_claims}`.

#### `AreaIndex`'s count methods are now `*_count` (#299)

Deprecated in 0.6.0 and removed before 1.0: `checkpoints`, `hints`, `confirmations` and
`pending_claims` become `checkpoint_count`, `hint_count`, `confirmation_count` and
`pending_claim_count`. They return the same counts. Nothing stops building until the removal, except
in builds that deny warnings.

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

### S3. `Image::read_sectors`

#### `Image::read_sectors` returns `Result<(), Error>` (#303)

Changed in 0.6.0, with no deprecation first, because the name stays and only the return type changes.
A range past the image's last sector, formerly `Ok(false)`, is now
`Error::SectorRangeInvalid { lsn, count, sectors }`, and leaves the buffer as it was. An empty
buffer, formerly `InvalidArgument`, reads nothing and succeeds for any `lsn` up to the sector count.

Code that uses the `bool` directly gets a compile error. **Code that doesn't still builds, and
`cargo semver-checks` does not detect this, so search for every call.** For a read past the end:

| Code | Before | Now |
|---|---|---|
| `if image.read_sectors(..)? { .. }` | builds | compile error |
| `image.read_sectors(..)?;`, also into another error type | carries on with unread data | returns the error |
| `image.read_sectors(..).unwrap();` | carries on with unread data | panics |
| `.is_ok()`, `.ok().is_some()`, `matches!(_, Ok(_))` | true | false |
| `let _ = image.read_sectors(..);` | ignored, buffer intact | ignored, buffer intact |

The C library's `sb_disc_read_sectors` does not change.

Before:

```rust
if image.read_sectors(lsn, &mut buf)? {
    use_sectors(&buf);
} else {
    past_the_end();
}
```

After:

```rust
match image.read_sectors(lsn, &mut buf) {
    Ok(()) => use_sectors(&buf),
    Err(Error::SectorRangeInvalid { .. }) => past_the_end(),
    Err(e) => return Err(e),
}
```

### N3. Duplicate constants

`MAX_CHANNELS`, `dst::FL` and `audio::FRAME_BYTES_PER_CHANNEL`.

#### `dst::MAX_CHANNELS` and `dst::FL` give way to the `audio` constants (#293)

Deprecated in 0.6.0 and removed before 1.0. `audio::MAX_CHANNELS` is a `u8`, where
`dst::MAX_CHANNELS` was a `usize`, so a size needs a conversion. The values do not change. Nothing
stops building until the removal, except in builds that deny warnings.
`scarletbook_pcm::MAX_CHANNELS` and `scarletbook_pcm::FRAME_BYTES_PER_CHANNEL` stay, because that
crate does not depend on `scarletbook`.

Before:

```rust
let buf = vec![0u8; dst::FL * dst::MAX_CHANNELS];
```

After:

```rust
let buf = vec![0u8; audio::FRAME_BYTES_PER_CHANNEL * usize::from(audio::MAX_CHANNELS)];
```

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

ABI 2 is the 1.0 ABI, so the header and the ABI do not change, and `SB_ABI_VERSION` stays 2. The
shared library's name now carries the ABI version on Linux and macOS.

#### `libscarletbook_ffi.so` is now `libscarletbook_ffi.so.2` (#294)

Changed in 0.6.0, and breaking for how a program is packaged, not for its source. No C source
changes, and neither do archive file names. In an archive's `lib/`, the versioned name is now the
file and the unversioned name a symlink to it:

| | The file, and the name a new program records | Symlink |
|---|---|---|
| Linux | `libscarletbook_ffi.so.2` (the SONAME) | `libscarletbook_ffi.so` |
| macOS | `libscarletbook_ffi.2.dylib` (install name `@rpath/libscarletbook_ffi.2.dylib`, compatibility version 2.0.0) | `libscarletbook_ffi.dylib` |

Static linking and Windows do not change: `scarletbook_ffi.dll` keeps its name, and
`sb_abi_version()` is the only ABI check there.

A program you **relink** still uses `-lscarletbook_ffi`, but needs the versioned file at run time.
Shipping only a file called `libscarletbook_ffi.so` fails with `libscarletbook_ffi.so.2: cannot
open shared object file`.

Before:

```sh
cc app.c -I$PKG/include -L$PKG/lib -lscarletbook_ffi -Wl,-rpath,'$ORIGIN'
cp $PKG/lib/libscarletbook_ffi.so "$DEST/"
```

After:

```sh
cc app.c -I$PKG/include -L$PKG/lib -lscarletbook_ffi -Wl,-rpath,'$ORIGIN'
cp $PKG/lib/libscarletbook_ffi.so.2 "$DEST/"
```

A program you **have not relinked** recorded the unversioned name, so it needs the symlink kept next
to the library. With only `libscarletbook_ffi.so.2` it fails with `libscarletbook_ffi.so: cannot open
shared object file`. A program built against the 0.5.8 archive runs with file and symlink on glibc
Linux, with the same output; musl and macOS were not tried with such a program.

A program that **loads the library by path**, as with ctypes, still works under the old name while
the symlink is there. Move to the versioned name: it is the real file, so it survives packaging
that drops symlinks.

Before:

```python
lib = ctypes.CDLL("/path/to/libscarletbook_ffi.so")
```

After:

```python
lib = ctypes.CDLL("/path/to/libscarletbook_ffi.so.2")  # libscarletbook_ffi.2.dylib on macOS
```
