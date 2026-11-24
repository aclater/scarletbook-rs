<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
[scarletbook](../README.md) › [Documentation](README.md) › Rust guide

# Rust guide

Using the `scarletbook` crate from Rust. For C and other languages, see the
[C ABI guide](c-abi.md); for what stays compatible between releases, see
[Stability and versions](stability.md).

## Installing

The crates are on crates.io: [`scarletbook`](https://crates.io/crates/scarletbook) (the
reader), [`scarletbook-pcm`](https://crates.io/crates/scarletbook-pcm) (DSD to PCM;
decommissioned ([#398](https://github.com/aclater/scarletbook-rs/issues/398)): frozen at 0.7.0
on crates.io, no further releases; consumers convert DSD to PCM in their own crate instead) and
[`scarletbook-ffi`](https://crates.io/crates/scarletbook-ffi) (the C ABI, from 0.5.8, for Rust programs
that build the C library themselves; C users take the [release assets](releases.md#release-assets)).

```sh
cargo add scarletbook
cargo add scarletbook-pcm   # only for PCM output; decommissioned, frozen at 0.7.0 (#398)
```

```rust
use std::{fs::File, io::BufReader};
use scarletbook::{Disc, Slot};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut disc = Disc::open(BufReader::new(File::open("disc.iso")?))?;
    let mut bytes = 0;
    for frame in disc.track_audio(Slot::TwoChannel, 1)? {
        bytes += frame?.data.len(); // interleaved DSD, DST already decoded
    }
    println!("track 1: {bytes} bytes of DSD");
    Ok(())
}
```

**Byte stability.** `scarletbook-pcm` is decommissioned
([#398](https://github.com/aclater/scarletbook-rs/issues/398)): frozen at 0.7.0 on crates.io,
no further releases (outside the 1.0 freeze, see [Stability](stability.md#stability)). So the
PCM bytes that `scarletbook-pcm` (and the C library's `sb_pcm_*`) produce stay those of 0.7.0;
the decoded DSD bytes are fixed by the specification. A plain `scarletbook-pcm = "0.7"`
resolves to 0.7.0, as does the exact pin:

```toml
scarletbook-pcm = "=0.7.0"
```

## Using the library

```rust
use std::{fs::File, io::BufReader};
use scarletbook::{Disc, Slot};

let mut disc = Disc::open(BufReader::new(File::open("disc.iso")?))?;
for frame in disc.track_audio(Slot::TwoChannel, 1)? {
    let frame = frame?; // C x 4704 bytes of interleaved DSD, DST already decoded
}
// Any frame range [first, end) of an area's timeline, pauses included: here the whole area,
// then track 2 with the pause after it.
let area = disc.toc().area(Slot::TwoChannel).ok_or(scarletbook::Error::NoSuchArea(Slot::TwoChannel))?;
let playtime = area.playtime_frames();
for frame in disc.range_audio(Slot::TwoChannel, 0, playtime)? {
    let frame = frame?;
}
let (first, end) = disc.track_range_with_pause(Slot::TwoChannel, 2)?;
for frame in disc.range_audio(Slot::TwoChannel, first, end)? {
    let frame = frame?;
}
```

### Stored content (DSDIFF passthrough)

To copy DST frames unchanged (for example into a DSDIFF file), read positions without
decoding them (specification 4, 4.6.1):

```rust
use scarletbook::FrameContent;

// A track's or range's positions as stored: DST bytes, plain DSD bytes, or missing.
let mut frames = disc.range_frames(Slot::TwoChannel, first, end)?;
while let Some(frame) = frames.next() {
    let frame = frame?;
    match &frame.content {
        FrameContent::Dst(bytes) => { /* the DST frame as stored */ }
        FrameContent::Dsd(bytes) => { /* C x 4704 plain bytes */ }
        FrameContent::Missing => { /* no frame: decoded reads fill it with 0x69 */ }
    }
    // Or one DST frame for every position, plain and missing ones included, that decodes
    // to exactly the position's audio.
    let dst = frames.dst_equivalent(&frame.content);
}
let missing = frames.missing_frames();
// Every accepted frame of the area, in frame number order, frames past the play time
// included; positions not listed are missing.
for frame in disc.stored_frame_list(Slot::TwoChannel)? {
    let frame = frame?;
}
```

`Disc::track_frames` does the same for a track. Positions, missing frames and malformed
sectors are exactly those of the decoded reads; invalid DST frames show only when decoded.
In C: `sb_raw_frames_open`, `sb_raw_frames_read`, `sb_raw_frames_read_dst_equivalent` and
`sb_stored_frame_list_open`, with caller buffers of at most `SB_MAX_FRAME_BYTES`.

### DSF and DSDIFF files

`scarletbook::output` (the default `output` feature) lays out a track, a track with pause or
any frame range as a DSF file, with an ID3v2.4 tag, or a DSDIFF file, with plain DSD or the
DST frames unchanged, markers and text, or as an edited master (specification 8). The file's
size and every byte that isn't audio come from the TOC alone, so a filesystem can list the
file before reading any audio, and produce any byte range of it:

```rust
use scarletbook::output::{Format, OutputOptions};

let (first, end) = disc.track_range_with_pause(Slot::TwoChannel, 1)?;
let layout = disc.output_layout(Slot::TwoChannel, first, end, Format::Dsf, &OutputOptions::default())?;
let size = layout.size();
let mut buf = vec![0; 65_536];
let n = layout.read_at(&mut disc, offset, &mut buf)?;
// Or with positions from a cache of your own:
let needed = layout.positions_for(offset..offset + buf.len() as u64);
let n = layout.fill(offset, &mut buf, &mut my_source)?;
```

From 0.7.0 on, `positions_for` gives only the positions of the bytes asked for (#307), not
those of every 4096 x C-byte DSF row the bytes touch, as 0.6.2 did. A position cache that
fetches what each request is missing can then fetch the rest of a row on a later request,
which costs one more fetch and seek: sacdfs measured +3.6 % image reads for the head and
tail of every file of a library (#431). To fetch as 0.6.2 did, round the range out to
whole rows of the `SoundData` region (`Layout::regions`) before calling `positions_for`.
On synthetic images, that gives 0.6.2's fetches, reads and bytes exactly.

A DST file's size needs the frames' lengths too: `DstLengths::scan` reads them without
decoding (they can be saved and loaded), and `Disc::output_layout_dst` lays the file out
from them. Every point where the formats allow more than one valid output (8.6, "writer's
choices") is a field of `OutputOptions` with a documented default; `TagSet::from_toc` gives
the tag items, which the caller may change. In C: `sb_output_options_*`, `sb_tags_*`,
`sb_dst_lengths_*` and `sb_output_layout_*` (see [Output files](c-abi.md#output-files)).

The departures from the DSDIFF document that some existing programs make (8.7: D1, `CMPR`'s
size counting its pad byte; D2, text as ISO 8859-1) are made only when named, in
`OutputOptions::dsdiff_departures` or with `SB_OUTPUT_OPTION_DSDIFF_DEPARTURE_D1` and `_D2`.
A file written with one is not a conforming DSDIFF file.

**An area's tracks as separate files** take no writer of their own, only a loop over these
calls: use `Disc::continue_range_audio` from track to track, since a plain `range_audio` per
track rescans the area (more than 2x slower measured on a 16-track stereo area). That
sequential loop gains nothing over one file. Workers gain on real storage, in two forms:

- **Seek index first:** the bytes are exact.
- **Speculative with a verifying pass:** the bytes are provisional when written (specification
  4, 4.2 item 8). They are exact only once the pass is complete and every reading is exact or
  confirmed, and any other reading's file is rewritten.

Workers reading speculatively with no verifying pass are a speed figure only: nothing
establishes their bytes, and matching hashes on some images don't. The `scarletbook::output`
documentation has the code for each form. The figures are on
[#202](https://github.com/aclater/scarletbook-rs/issues/202): they come from one machine, so
measure on yours.

### Seeking within an area

A range far into an area normally costs a scan of the area up to it: which frames a range
holds depends on every audio sector before it (specification 4, 4.5), so an exact reader
cannot skip ahead without having read them once. A **seek index** keeps the reading state
every 64 sectors, so that once it covers a range, opening that range reads only a few dozen
sectors before the range's own, wherever it lies (specification 4, 4.2 item 7). It is off
by default:

```rust
use scarletbook::{AreaIndex, IndexOptions};

disc.enable_seek_index(Slot::TwoChannel, IndexOptions::new())?;
// Reads build it as they go; or build it at once, in steps (one pass over the area):
while !disc.extend_seek_index(Slot::TwoChannel, 64 * 1024)? {}
// Every track and range cursor of the area now starts from it, with the same frames,
// counts and warnings as without it.
let audio = disc.range_audio(Slot::TwoChannel, first, end)?;
// Keep it for the next time this image is opened (about 24 bytes per 64 sectors).
let saved: Vec<u8> = disc.seek_index(Slot::TwoChannel).map(|i| i.to_bytes()).unwrap_or_default();
// ... later: from_bytes checks the bytes, set_seek_index the area.
let index = AreaIndex::from_bytes(area, disc.toc().sectors, &saved, scarletbook::seek::DEFAULT_INLINE_BUDGET)?;
disc.set_seek_index(Slot::TwoChannel, index)?;
```

A saved index is checked for safety (bounds, invariants, checksum, the area and image size
it belongs to) and, as it is used, against the image's sectors (a digest per checkpoint):
a mismatched index is discarded with a warning in the log and the area scanned from its
start. It is not proof against an index forged to match: keep it with the image it
describes. Every disc of one image (`Disc::share`, or `clone` of a disc read through a
`ReadAtReader`) uses and grows the same index, so to build it in the background, call
`extend_seek_index` on a clone in another thread while the others read; `AreaIndex::merge`
joins indexes kept apart, for example one loaded and one built.

A first seek into an area without a seek index still reads the area up to the range, which
over a network share can take seconds. A caller that accepts provisional positions can opt in
to **provisional reading** (specification 4, 4.2 item 8): `disc.set_seek_policy(slot,
SeekPolicy::Speculative(SpeculativeOptions::new()))` makes far ranges start after a short
search of the frame timecodes, a few small reads wherever the range lies, and read the range
in reads sized to it (one for a range of up to 1 MiB). Their positions are
marked provisional (`speculation()`): they equal a straight read on well-formed discs, but no
reader can know that without reading the sectors before the range, and on hostile images they
can differ (see `SeekPolicy`). Their malformed counts begin at the provisional start
(`claim_lsn()`). With a seek index enabled for the area, each provisional reading is
checked once the index reaches it (build it with `disc.verify_speculation(slot, sectors)`,
for example on a clone in another thread): confirmed, or refuted with the frame from which its
positions are exact; the cursor's `speculation()` and `disc.take_seek_events(slot)` report it,
so that a caller can drop and re-read what it served. A provisional reading the library has
its own reasons to doubt is marked **suspect** (`suspect()`: an area whose TOC places its
audio past the image's end or over the other area's sectors, or frame numbers that decreased
between the sectors its search read; 4.2 item 8), and
`SpeculativeOptions::with_exact_when_suspect(true)` reads those exactly instead. The mark is
informative: it does not change how the reading is settled.

The seek index also keeps **seek hints** (the locator samples of specification 4, 4.2 item 8):
sectors its cursors read with the frame number of their first frame record, every 8 frames,
at most 2 048 (8 bytes each, `IndexOptions::with_hints`). They are saved with the index
(`AreaIndex::to_bytes`), so that after a remount or a restart a provisional reading near a
range read before begins at the hint below it and reads through the hint past it: **one read
call**, the range's sectors and a few more, with no search and no pass from the area's start.
Its positions are provisional all the same; a hint only chooses where to begin. A hint whose
sector no longer has its frame record (a stale index, or one of another image) is dropped
with a warning, and the cursor searches instead; an index saved by a disc carries a
fingerprint of its TOC, and a disc with another TOC refuses it (`Disc::set_seek_index`). With
a complete saved index, an exact read starts with one read sized to the range.

When a pass (`disc.verify_speculation`) has confirmed a provisional reading, the index keeps
that confirmation with the hint (4.2 item 9): after a remount, a reading from that hint for
the same or a later first frame is reported confirmed at once, still in one read call, if the
index is bound to the disc (key and TOC fingerprint) and the hint's sector has the bytes the
pass read. `AreaIndex::without_checkpoints` keeps only the hints, confirmations and
fingerprint: a few kilobytes per area to save, for a caller that keeps many discs. Saved
confirmations, like saved exact checkpoints, are trusted for any image whose TOC fingerprint
and key match, so a re-rip with different audio bytes would get confirmed readings that
differ: key saved indexes by the image file's identity (path, size and modification time, or
a content hash) as well.

## Features

`parallel` decodes DST frames on rayon's thread pool, ahead of the caller: the
current pool by default, or the pool chosen for a cursor with
`TrackAudioCursor::set_decode_on(DecodeOn::Pool(pool))`, whichever thread reads it
(`DecodeOn::CallingThread` keeps a cursor off every pool).

With `parallel`, rayon 1 is part of the public Rust API, and that is a stated decision (#39):
`DecodeOn::Pool` takes an `Arc<rayon::ThreadPool>`, not a wrapper of this library's. rayon 1 is
stable, and a wrapper would add interface for a problem that may never come. The cost is that a
rayon 2 would need a new major version of `scarletbook`. A program's pool must come from rayon 1.

The names that differ between the Rust API and the C ABI are listed under
[Rust and C names](c-abi.md#rust-and-c-names).

## See also

- [Using the C library](c-abi.md)
- [Stability and versions](stability.md)
- [Performance and benchmarks](performance.md)
- [Documentation index](README.md)
