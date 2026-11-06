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

Not written yet.

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
