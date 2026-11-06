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

Not written yet.

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
