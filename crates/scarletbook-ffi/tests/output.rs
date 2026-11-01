// SPDX-License-Identifier: MIT OR Apache-2.0

//! DSF and DSDIFF output through the C ABI (issue #131): every layout equals the Rust
//! API's for the same options, byte for byte, through `sb_output_layout_read` and through
//! `sb_output_layout_positions` and `sb_output_layout_fill`; options, tag items and DST
//! lengths round-trip; and refusals have their status codes.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::ffi::{CStr, CString, c_char};
use std::io::Cursor;
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

use scarletbook::output::{
    CallerChunk, Comment, CommentKind, DsfBitsPerSample, DstIndexOffset, DstLengths, Format, InvalidDst,
    Layout, MarkerText, OutputOptions, TagItem, TagSet, Tags,
};
use scarletbook::{Disc, Slot};
use scarletbook_ffi::*;

/// The core crate's builder of small synthetic images.
#[path = "../../scarletbook/tests/common/mod.rs"]
mod common;

/// Calls into the C ABI. Every call in this file passes live handles (or NULL where the
/// test checks the refusal), pointers valid for the lengths given, and NUL-terminated
/// strings.
macro_rules! c {
    ($e:expr) => {{
        // SAFETY: as the macro's documentation says, for each use.
        unsafe { $e }
    }};
}

fn corpus(names: &[&str]) -> Vec<Vec<u8>> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/images");
    names.iter().map(|n| std::fs::read(dir.join(format!("{n}.iso"))).unwrap()).collect()
}

fn open(bytes: &[u8]) -> *mut SbDisc {
    let mut d = ptr::null_mut();
    assert_eq!(c!(sb_disc_open_memory(bytes.as_ptr(), bytes.len(), &mut d)), SbStatus::Ok);
    d
}

fn options() -> *mut SbOutputOptions {
    let mut o = ptr::null_mut();
    assert_eq!(c!(sb_output_options_new(&mut o)), SbStatus::Ok);
    o
}

fn set(o: *const SbOutputOptions, option: SbOutputOption, value: i64) -> SbStatus {
    c!(sb_output_options_set(o, option as u32, value))
}

fn info(l: *const SbOutputLayout) -> SbOutputLayoutInfo {
    let mut i = SbOutputLayoutInfo::default();
    assert_eq!(c!(sb_output_layout_info(l, &mut i)), SbStatus::Ok);
    i
}

/// The whole file of a C layout, read through `sb_output_layout_read` in pieces.
fn read(l: *const SbOutputLayout, d: *const SbDisc, piece: usize) -> Vec<u8> {
    let size = info(l).size as usize;
    let mut f = vec![0; size];
    let mut at = 0;
    while at < size {
        let mut n = 0;
        let len = piece.min(size - at);
        assert_eq!(
            c!(sb_output_layout_read(l, d, at as u64, f[at..].as_mut_ptr(), len, &mut n)),
            SbStatus::Ok
        );
        assert_eq!(n, len);
        at += n;
    }
    f
}

/// The whole file of a Rust layout.
fn rust_file(disc: &mut Disc<Cursor<Vec<u8>>>, l: &Layout) -> Vec<u8> {
    let mut f = vec![0; l.size() as usize];
    l.read_at(disc, 0, &mut f).unwrap();
    f
}

/// Fills `[at, at + len)` of a C layout through `sb_output_layout_fill`, with the
/// positions `sb_output_layout_positions` names, from the Rust API.
fn fill(
    l: *const SbOutputLayout,
    disc: &mut Disc<Cursor<Vec<u8>>>,
    slot: Slot,
    at: u64,
    len: usize,
) -> Vec<u8> {
    let (mut first, mut end) = (0, 0);
    assert_eq!(c!(sb_output_layout_positions(l, at, len as u64, &mut first, &mut end)), SbStatus::Ok);
    let channels = disc.toc().area(slot).unwrap().channel_count;
    let (content, equivalents): (Vec<Vec<u8>>, Vec<Vec<u8>>) = if first < end {
        let c = disc.range_audio(slot, first, end).unwrap().map(|f| f.unwrap().data).collect();
        let e = disc
            .range_frames(slot, first, end)
            .unwrap()
            .map(|f| f.unwrap().content.dst_equivalent(channels).unwrap().into_owned())
            .collect();
        (c, e)
    } else {
        (Vec::new(), Vec::new())
    };
    let data: Vec<SbPositionData> = content
        .iter()
        .zip(&equivalents)
        .enumerate()
        .map(|(i, (c, e))| SbPositionData {
            content: c.as_ptr(),
            content_len: c.len(),
            dst_equivalent: e.as_ptr(),
            dst_equivalent_len: e.len(),
            number: first + i as u32,
            reserved: 0,
        })
        .collect();
    let mut out = vec![0; len];
    let mut n = 0;
    assert_eq!(
        c!(sb_output_layout_fill(l, at, out.as_mut_ptr(), len, data.as_ptr(), data.len(), &mut n)),
        SbStatus::Ok
    );
    out.truncate(n);
    out
}

#[test]
#[cfg_attr(miri, ignore = "corpus images are too large for Miri")]
fn layouts_equal_the_rust_api() {
    for bytes in corpus(&["dst-features-2ch-6ch", "album-set-disc-2-of-3"]) {
        let d = open(&bytes);
        let mut disc = Disc::open(Cursor::new(bytes.clone())).unwrap();
        for area in disc.toc().areas.clone() {
            let slot = match area.slot {
                Slot::TwoChannel => 0,
                Slot::Multichannel => 1,
            };
            let t = &area.tracks[0];
            let (first, end) = (t.start_frame, t.end_frame());
            // DSF with options set through the C ABI and the same options in Rust.
            let o = options();
            assert_eq!(set(o, SbOutputOption::DsfBitsPerSample, 8), SbStatus::Ok);
            assert_eq!(set(o, SbOutputOption::Id3Footer, 0), SbStatus::Ok);
            assert_eq!(set(o, SbOutputOption::Id3Padding, 7), SbStatus::Ok);
            let lang = CString::new("eng").unwrap();
            assert_eq!(
                c!(sb_output_options_set_string(o, SbOutputString::Id3Language as u32, lang.as_ptr())),
                SbStatus::Ok
            );
            let mut rust = OutputOptions::default().with_dsf_bits_per_sample(DsfBitsPerSample::Eight);
            rust.id3.footer = false;
            rust.id3.padding = 7;
            rust.id3.language = *b"eng";
            let mut l = ptr::null_mut();
            assert_eq!(
                c!(sb_output_layout_new(d, slot, first, end, SbOutputFormat::Dsf as u32, o, &mut l)),
                SbStatus::Ok
            );
            let r = disc.output_layout(area.slot, first, end, Format::Dsf, &rust).unwrap();
            let want = rust_file(&mut disc, &r);
            assert_eq!(read(l, d, 5000), want);
            let i = info(l);
            assert_eq!(
                (i.size, i.first, i.end, i.source_track, i.input, i.format),
                (want.len() as u64, first, end, r.source_track().map_or(0, u32::from), 0, 0)
            );
            assert_eq!(i.regions as usize, r.regions().len());
            let mut region = SbOutputRegion::default();
            assert_eq!(c!(sb_output_layout_region(l, 1, &mut region)), SbStatus::Ok);
            assert_eq!(
                (region.kind, region.offset, region.len),
                (1, r.regions()[1].offset, r.regions()[1].len)
            );
            assert_eq!(c!(sb_output_layout_region(l, i.regions, &mut region)), SbStatus::InvalidArgument);
            for (at, len) in [(0, 100), (90, 9000), (want.len() as u64 - 10, 10)] {
                assert_eq!(fill(l, &mut disc, area.slot, at, len), want[at as usize..at as usize + len]);
            }
            c!(sb_output_layout_free(l));

            // DSDIFF with DST: lengths scanned, saved and loaded, CRCs and the other index offset.
            let mut lengths = ptr::null_mut();
            assert_eq!(c!(sb_dst_lengths_scan(d, slot, first, end, 1, &mut lengths)), SbStatus::Ok);
            let mut count = 0;
            assert_eq!(
                c!(sb_dst_lengths_get(lengths, ptr::null_mut(), 0, &mut count)),
                SbStatus::BufferTooSmall
            );
            let mut saved = vec![0u32; count];
            assert_eq!(c!(sb_dst_lengths_get(lengths, saved.as_mut_ptr(), count, &mut count)), SbStatus::Ok);
            let rust_lengths =
                DstLengths::scan(&mut disc, area.slot, first, end, InvalidDst::Replaced).unwrap();
            assert_eq!(saved, rust_lengths.lengths());
            let mut loaded = ptr::null_mut();
            assert_eq!(
                c!(sb_dst_lengths_load(d, slot, first, end, 1, saved.as_ptr(), count, &mut loaded)),
                SbStatus::Ok
            );
            assert_eq!(set(o, SbOutputOption::DsdiffDstCrc, 1), SbStatus::Ok);
            assert_eq!(set(o, SbOutputOption::DsdiffDstIndexOffset, 1), SbStatus::Ok);
            let rust = rust.with_dsdiff_dst_crc(true).with_dsdiff_dst_index_offset(DstIndexOffset::FrameData);
            let mut l = ptr::null_mut();
            assert_eq!(c!(sb_output_layout_new_dst(d, loaded, o, &mut l)), SbStatus::Ok);
            let r = disc.output_layout_dst(&rust_lengths, &rust).unwrap();
            let want = rust_file(&mut disc, &r);
            assert_eq!(read(l, d, 1 << 20), want);
            assert_eq!(info(l).input, 2);
            assert_eq!(
                fill(l, &mut disc, area.slot, 0, want.len().min(30_000)),
                want[..want.len().min(30_000)]
            );
            c!({
                sb_output_layout_free(l);
                sb_dst_lengths_free(lengths);
                sb_dst_lengths_free(loaded);
                sb_output_options_free(o);
            })
        }
        c!(sb_disc_free(d));
    }
}

fn get(t: *const SbTags, item: SbTagItem, index: u32) -> Option<String> {
    let mut buf = [0 as c_char; 256];
    let mut len = 0;
    match c!(sb_tags_get(t, item as u32, index, buf.as_mut_ptr(), buf.len(), &mut len)) {
        SbStatus::Ok => Some(c!(CStr::from_ptr(buf.as_ptr())).to_str().unwrap().to_owned()),
        SbStatus::Absent => None,
        s => panic!("{s:?}"),
    }
}

#[test]
#[cfg_attr(miri, ignore = "corpus images are too large for Miri")]
fn tags_and_their_options() {
    let bytes = corpus(&["album-set-disc-2-of-3"]).pop().unwrap();
    let d = open(&bytes);
    let mut disc = Disc::open(Cursor::new(bytes.clone())).unwrap();
    let toc = disc.toc().clone();
    let area = &toc.areas[0];
    let mut t = ptr::null_mut();
    assert_eq!(c!(sb_tags_from_toc(d, 0, 1, ptr::null(), &mut t)), SbStatus::Ok);
    let rust = TagSet::from_toc(&toc, area, area.track(1), &OutputOptions::default());
    assert_eq!(
        get(t, SbTagItem::DiscNumber, 0).as_deref(),
        rust.get(TagItem::DiscNumber).first().map(String::as_str)
    );
    assert_eq!(get(t, SbTagItem::TrackNumber, 0).as_deref(), Some("1"));
    let v = CString::new("Ünïcode").unwrap();
    let w = CString::new("second").unwrap();
    c!({
        assert_eq!(sb_tags_set(t, SbTagItem::Performer as u32, v.as_ptr()), SbStatus::Ok);
        assert_eq!(sb_tags_push(t, SbTagItem::Performer as u32, w.as_ptr()), SbStatus::Ok);
        assert_eq!(sb_tags_set_custom(t, w.as_ptr(), v.as_ptr()), SbStatus::Ok);
        assert_eq!(sb_tags_remove(t, SbTagItem::Isrc as u32), SbStatus::Ok);
        assert_eq!(sb_tags_set(t, 99, v.as_ptr()), SbStatus::InvalidArgument);
        assert_eq!(sb_tags_set(t, 0, ptr::null()), SbStatus::InvalidArgument);
    });
    let mut n = 0;
    assert_eq!(c!(sb_tags_count(t, SbTagItem::Performer as u32, &mut n)), SbStatus::Ok);
    assert_eq!(n, 2);
    assert_eq!(get(t, SbTagItem::Performer, 1).as_deref(), Some("second"));
    assert_eq!(get(t, SbTagItem::Performer, 2), None);
    // The set's tag in a DSF file, as in Rust.
    let o = options();
    assert_eq!(c!(sb_output_options_set_tags(o, t)), SbStatus::Ok);
    let mut rust_set = rust.clone();
    rust_set.set(TagItem::Performer, "Ünïcode");
    rust_set.push(TagItem::Performer, "second");
    rust_set.set_custom("second", "Ünïcode");
    rust_set.remove(TagItem::Isrc);
    let tr = area.track(1).unwrap();
    let mut l = ptr::null_mut();
    assert_eq!(c!(sb_output_layout_new(d, 0, tr.start_frame, tr.end_frame(), 0, o, &mut l)), SbStatus::Ok);
    let r = disc
        .output_layout(
            Slot::TwoChannel,
            tr.start_frame,
            tr.end_frame(),
            Format::Dsf,
            &OutputOptions::default().with_tags(Tags::Set(rust_set)),
        )
        .unwrap();
    assert_eq!(read(l, d, 4096), rust_file(&mut disc, &r));
    c!({
        sb_output_layout_free(l);
        sb_tags_free(t);
        sb_output_options_free(o);
        sb_disc_free(d);
    })
}

#[test]
fn options_are_checked() {
    let o = options();
    for (option, ok, bad) in [
        (SbOutputOption::DsfBitsPerSample, 1, 2),
        (SbOutputOption::DsfUnspecifiedChannelType, 7, -1),
        (SbOutputOption::DsdiffAbsoluteStart, -1, -2),
        (SbOutputOption::DsdiffDstIndexOffset, 1, 2),
        (SbOutputOption::SourceTrack, 255, 256),
        (SbOutputOption::DsdiffArtistTitle, -1, 2),
        (SbOutputOption::DsdiffTextReplacement, -1, 0x7F),
        (SbOutputOption::TextSource, 2, 3),
        (SbOutputOption::Genres, 7, 8),
        (SbOutputOption::Id3Encoding, 4, 5),
        (SbOutputOption::Id3ControlReplacement, 0x2E, 0xD800),
        (SbOutputOption::Id3AlbumArtist, 2, 3),
        (SbOutputOption::Id3Date, 1, 2),
        (SbOutputOption::Tags, 1, 2),
    ] {
        assert_eq!(set(o, option, ok), SbStatus::Ok, "{option:?}");
        assert_eq!(set(o, option, bad), SbStatus::InvalidArgument, "{option:?}");
    }
    assert_eq!(set(o, SbOutputOption::Id3Date, 0), SbStatus::Ok);
    assert_eq!(c!(sb_output_options_set(o, 33, 0)), SbStatus::InvalidArgument);
    let order = [4u32, 3, 2, 1, 0];
    let bad = [0u32, 0, 0, 0, 0];
    let ids = c"TDRLTIT2";
    let marker_text = c"m";
    let comment_text = c"note";
    c!({
        assert_eq!(
            sb_output_options_set_order(o, SbOutputOrder::Property as u32, order.as_ptr(), 5),
            SbStatus::Ok
        );
        assert_eq!(
            sb_output_options_set_order(o, SbOutputOrder::Property as u32, order.as_ptr(), 4),
            SbStatus::InvalidArgument
        );
        assert_eq!(
            sb_output_options_set_order(o, SbOutputOrder::Info as u32, order[1..].as_ptr(), 4),
            SbStatus::Ok
        );
        assert_eq!(
            sb_output_options_set_order(o, SbOutputOrder::Local as u32, bad.as_ptr(), 3),
            SbStatus::Ok
        );
        assert_eq!(
            sb_output_options_set_order(o, SbOutputOrder::Local as u32, ptr::null(), 3),
            SbStatus::InvalidArgument
        );
        assert_eq!(sb_output_options_set_order(o, 3, order.as_ptr(), 5), SbStatus::InvalidArgument);
        assert_eq!(sb_output_options_set_id3_frame_order(o, ids.as_ptr(), 2), SbStatus::Ok);
        assert_eq!(sb_output_options_set_channel_numbers(o, [7u16, 8].as_ptr(), 2), SbStatus::Ok);
        assert_eq!(
            sb_output_options_set_string(o, SbOutputString::Id3Language as u32, c"en".as_ptr()),
            SbStatus::InvalidArgument
        );
        assert_eq!(
            sb_output_options_set_string(o, SbOutputString::Id3Split as u32, ptr::null()),
            SbStatus::Ok
        );
        assert_eq!(sb_output_options_set_string(o, 8, ptr::null()), SbStatus::InvalidArgument);
        let m = SbMarker { text: marker_text.as_ptr(), frame: 0, kind: 3, track_flags: 0, reserved: 0 };
        assert_eq!(sb_output_options_add_marker(o, &m), SbStatus::InvalidArgument);
        let m = SbMarker { kind: 4, ..m };
        assert_eq!(sb_output_options_add_marker(o, &m), SbStatus::Ok);
        assert_eq!(sb_output_options_clear_markers(o), SbStatus::Ok);
        let c = SbComment {
            text: comment_text.as_ptr(),
            year: 2026,
            kind: 4,
            reference: 0,
            month: 1,
            day: 1,
            hour: 0,
            minutes: 0,
        };
        assert_eq!(sb_output_options_add_comment(o, &c), SbStatus::InvalidArgument);
        let c = SbComment { kind: 3, ..c };
        assert_eq!(sb_output_options_add_comment(o, &c), SbStatus::Ok);
        assert_eq!(sb_output_options_clear_comments(o), SbStatus::Ok);
        let too_big = vec![0u8; scarletbook::output::MAX_TAG_BYTES + 1];
        assert_eq!(
            sb_output_options_set_raw_tag(o, too_big.as_ptr(), too_big.len()),
            SbStatus::InvalidArgument
        );
        assert_eq!(sb_output_options_set(ptr::null(), 0, 1), SbStatus::InvalidArgument);
        sb_output_options_free(o);
        sb_output_options_free(ptr::null_mut());
        sb_tags_free(ptr::null_mut());
        sb_dst_lengths_free(ptr::null_mut());
        sb_output_layout_free(ptr::null_mut());
    });
    // The status names.
    let name = |s: i32| c!(CStr::from_ptr(sb_status_name(s))).to_str().unwrap();
    assert_eq!(name(SbStatus::OutputLayoutUnspecified as i32), "output-layout-unspecified");
    assert_eq!(name(SbStatus::NotEditedMaster as i32), "not-edited-master");
}

#[test]
#[cfg_attr(miri, ignore = "corpus images are too large for Miri")]
fn refusals() {
    // An area with an unspecified layout, and an edited master of tracks too short.
    let images = corpus(&["dst-features-4ch", "dst-features-2ch-6ch"]);
    let d = open(&images[1]);
    let mut l = ptr::null_mut();
    let o = options();
    assert_eq!(set(o, SbOutputOption::DsdiffEditedMaster, 1), SbStatus::Ok);
    let disc = Disc::open(Cursor::new(images[1].clone())).unwrap();
    let p = disc.toc().areas[0].playtime_frames();
    assert_eq!(c!(sb_output_layout_new(d, 0, 0, p, 1, o, &mut l)), SbStatus::NotEditedMaster);
    assert!(l.is_null());
    c!({
        assert_eq!(sb_output_layout_new(d, 0, 0, p, 2, ptr::null(), &mut l), SbStatus::InvalidArgument);
        assert_eq!(sb_output_layout_new(d, 0, 0, p, 3, ptr::null(), &mut l), SbStatus::InvalidArgument);
        assert_eq!(sb_output_layout_new(d, 0, 0, p + 1, 0, ptr::null(), &mut l), SbStatus::FrameRangeInvalid);
        sb_disc_free(d);
        sb_output_options_free(o);
    });
    let disc = Disc::open(Cursor::new(images[0].clone())).unwrap();
    if let Some(area) =
        disc.toc().areas.iter().find(|a| {
            a.channel_speakers().is_none() && disc.check_range(a.slot, 0, a.playtime_frames()).is_ok()
        })
    {
        let d = open(&images[0]);
        let slot = if area.slot == Slot::TwoChannel { 0 } else { 1 };
        assert_eq!(
            c!(sb_output_layout_new(d, slot, 0, area.playtime_frames(), 0, ptr::null(), &mut l)),
            SbStatus::OutputLayoutUnspecified
        );
        c!(sb_disc_free(d));
    }
}

/// Lower-case hex of a digest.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The files `output-smoke.txt` lists, which `.github/scripts/clib-smoke.py` checks the
/// packaged C library against: the Rust API's output for track 1 of the 2-channel area of
/// corpus image dst-features-2ch-6ch, with the default options, as DSF and as DSDIFF with
/// the DST frames unchanged.
#[test]
#[cfg_attr(miri, ignore = "corpus images are too large for Miri")]
fn smoke_files_are_the_rust_output() {
    use sha2::{Digest, Sha256};
    let bytes = corpus(&["dst-features-2ch-6ch"]).remove(0);
    let mut disc = Disc::open(Cursor::new(bytes)).unwrap();
    let t = &disc.toc().area(Slot::TwoChannel).unwrap().tracks[0];
    let (first, end) = (t.start_frame, t.end_frame());
    let options = OutputOptions::default();
    let dsf = disc.output_layout(Slot::TwoChannel, first, end, Format::Dsf, &options).unwrap();
    let lengths = DstLengths::scan(&mut disc, Slot::TwoChannel, first, end, InvalidDst::Stored).unwrap();
    let dst = disc.output_layout_dst(&lengths, &options).unwrap();
    let mut got = String::new();
    for (name, layout) in [("dsf", &dsf), ("dsdiff-dst", &dst)] {
        let f = rust_file(&mut disc, layout);
        got += &format!("{name} {first} {end} {} {}\n", f.len(), hex(&Sha256::digest(&f)));
    }
    let listed = include_str!("output-smoke.txt");
    let want: String = listed.lines().filter(|l| !l.starts_with('#')).map(|l| format!("{l}\n")).collect();
    assert_eq!(got, want, "update output-smoke.txt only if the output is meant to change");
}

/// A layout made through the C ABI equals the Rust API's, through `sb_output_layout_read`
/// and through `sb_output_layout_fill`, on a disc small enough for Miri: DSF, and DSDIFF
/// with `DIIN`, marker text and a comment.
#[test]
#[cfg_attr(miri, ignore = "rayon's crossbeam-epoch fails Stacked Borrows; run with Tree Borrows")]
fn small_disc_layouts_equal_the_rust_api() {
    let frames: Vec<common::PFrame> = (0..3u32)
        .map(|number| common::PFrame {
            number,
            dst: false,
            data: (0..2 * 4704).map(|i| (i as u32 * 7 + number * 13) as u8).collect(),
        })
        .collect();
    let tracks =
        vec![common::TrackSpec { start: 0, duration: 1 }, common::TrackSpec { start: 1, duration: 2 }];
    let bytes = common::Spec { channels: 2, playtime: 3, tracks, audio: common::pack(&frames) }.build();
    let d = open(&bytes);
    let mut disc = Disc::open(Cursor::new(bytes.clone())).unwrap();
    let text = CString::new("op\u{e9}").unwrap();
    let o = options();
    assert_eq!(set(o, SbOutputOption::DsdiffEditedMasterInfo, 1), SbStatus::Ok);
    assert_eq!(set(o, SbOutputOption::DsdiffMarkerText, 1), SbStatus::Ok);
    let comment = SbComment {
        text: text.as_ptr(),
        year: 2026,
        kind: 3,
        reference: 1,
        month: 11,
        day: 1,
        hour: 9,
        minutes: 30,
    };
    assert_eq!(c!(sb_output_options_add_comment(o, &comment)), SbStatus::Ok);
    let mut dated = Comment::new(CommentKind::FileHistory(1), "op\u{e9}");
    (dated.year, dated.month, dated.day, dated.hour, dated.minutes) = (2026, 11, 1, 9, 30);
    let rust = OutputOptions::default()
        .with_dsdiff_edited_master_info(true)
        .with_dsdiff_marker_text(MarkerText::TrackTitle)
        .with_dsdiff_comments(vec![dated]);
    for (format, sb) in [(Format::Dsf, SbOutputFormat::Dsf), (Format::DsdiffDsd, SbOutputFormat::DsdiffDsd)] {
        let layout = disc.output_layout(Slot::TwoChannel, 0, 3, format, &rust).unwrap();
        let want = rust_file(&mut disc, &layout);
        if format == Format::DsdiffDsd {
            // The options take effect: the comment, as text bytes (W26), and a track title.
            assert!(want.windows(4).any(|w| w == b"op?\0"));
            assert!(want.windows(4).any(|w| w == b"DIIN"));
        }
        let mut l = ptr::null_mut();
        assert_eq!(c!(sb_output_layout_new(d, 0, 0, 3, sb as u32, o, &mut l)), SbStatus::Ok);
        assert_eq!(info(l).size, want.len() as u64);
        assert_eq!(read(l, d, 5_000), want);
        let at = want.len() as u64 - 6_000;
        assert_eq!(fill(l, &mut disc, Slot::TwoChannel, at, 5_000), want[at as usize..][..5_000]);
        c!(sb_output_layout_free(l));
    }
    c!(sb_output_options_free(o));
    c!(sb_disc_free(d));
}

/// What [`free_during_read`] needs: the image, and the handles its callback frees.
struct Victim {
    bytes: Vec<u8>,
    layout: AtomicPtr<SbOutputLayout>,
    lengths: AtomicPtr<SbDstLengths>,
    /// The info the callback read from the layout it tried to free, once per read.
    sizes: std::sync::Mutex<Vec<u64>>,
}

/// A positional read that, when armed, frees the layout (in use by the read: a no-op) and
/// the DST lengths (in use by no call: freed), then checks that the layout is still live.
unsafe extern "C" fn free_during_read(
    user: *mut std::ffi::c_void,
    offset: u64,
    buf: *mut u8,
    len: usize,
) -> i64 {
    // SAFETY: `user` is the `Victim` given to sb_disc_open_callbacks_at, alive for the disc.
    let v = unsafe { &*user.cast::<Victim>() };
    let layout = v.layout.swap(ptr::null_mut(), Ordering::SeqCst);
    if !layout.is_null() {
        c!(sb_output_layout_free(layout));
        c!(sb_dst_lengths_free(v.lengths.swap(ptr::null_mut(), Ordering::SeqCst)));
        v.sizes.lock().unwrap().push(info(layout).size);
    }
    let Some(rest) = usize::try_from(offset).ok().and_then(|o| v.bytes.get(o..)) else { return 0 };
    let n = len.min(rest.len());
    // SAFETY: `buf` is valid for `len >= n` bytes of writes, by the callback's contract.
    unsafe { ptr::copy_nonoverlapping(rest.as_ptr(), buf, n) };
    n as i64
}

/// A read or log callback that frees the layout being read is refused (the free does
/// nothing), the read finishes, and the layout stays usable and freeable; the DST lengths a
/// layout was made from can be freed at any time. On the plain DSD area of a corpus image,
/// small enough for Miri.
#[test]
#[cfg_attr(miri, ignore = "rayon's crossbeam-epoch fails Stacked Borrows; run with Tree Borrows")]
fn freeing_handles_during_a_read() {
    // Built in, as Miri's isolation allows no file reads.
    let bytes = include_bytes!("../../../corpus/images/album-set-disc-2-of-3.iso").to_vec();
    let mut disc = Disc::open(Cursor::new(bytes.clone())).unwrap();
    let end = disc.toc().area(Slot::TwoChannel).unwrap().playtime_frames();
    let victim = Box::new(Victim {
        bytes,
        layout: AtomicPtr::new(ptr::null_mut()),
        lengths: AtomicPtr::new(ptr::null_mut()),
        sizes: std::sync::Mutex::new(Vec::new()),
    });
    let user = ptr::from_ref::<Victim>(&victim).cast_mut().cast();
    let mut d = ptr::null_mut();
    let size = victim.bytes.len() as u64;
    assert_eq!(c!(sb_disc_open_callbacks_at(Some(free_during_read), size, user, &mut d)), SbStatus::Ok);
    let o = options();
    for format in [Format::Dsf, Format::DsdiffDsd, Format::DsdiffDst] {
        let mut lengths = ptr::null_mut();
        assert_eq!(c!(sb_dst_lengths_scan(d, 0, 0, end, 0, &mut lengths)), SbStatus::Ok);
        let mut l = ptr::null_mut();
        let want = if format == Format::DsdiffDst {
            assert_eq!(c!(sb_output_layout_new_dst(d, lengths, o, &mut l)), SbStatus::Ok);
            let rust = DstLengths::scan(&mut disc, Slot::TwoChannel, 0, end, InvalidDst::Stored).unwrap();
            let layout = disc.output_layout_dst(&rust, &OutputOptions::default()).unwrap();
            rust_file(&mut disc, &layout)
        } else {
            let sb = if format == Format::Dsf { SbOutputFormat::Dsf } else { SbOutputFormat::DsdiffDsd };
            assert_eq!(c!(sb_output_layout_new(d, 0, 0, end, sb as u32, o, &mut l)), SbStatus::Ok);
            let layout =
                disc.output_layout(Slot::TwoChannel, 0, end, format, &OutputOptions::default()).unwrap();
            rust_file(&mut disc, &layout)
        };
        victim.layout.store(l, Ordering::SeqCst);
        victim.lengths.store(lengths, Ordering::SeqCst);
        // Armed: the first read callback of the next read frees both handles.
        assert_eq!(read(l, d, 1 << 20), want);
        assert!(victim.layout.load(Ordering::SeqCst).is_null(), "{format:?}: the callback ran");
        assert_eq!(victim.sizes.lock().unwrap().pop(), Some(want.len() as u64));
        // The layout is still live: it reads again, and frees.
        assert_eq!(read(l, d, 4_000), want);
        c!(sb_output_layout_free(l));
    }
    c!(sb_output_options_free(o));
    c!(sb_disc_free(d));
    drop(victim);
}

/// `sb_output_layout_fill` refuses a buffer that overlaps the positions array or a
/// position's content or DST equivalent, and accepts one that only touches them.
#[test]
fn overlapping_fill_buffers_are_refused() {
    let frame = |n: u32| -> Vec<u8> { (0..2 * 4704).map(|i| (i as u32 * 7 + n * 13) as u8).collect() };
    let frames: Vec<common::PFrame> =
        (0..3).map(|number| common::PFrame { number, dst: false, data: frame(number) }).collect();
    let tracks = vec![common::TrackSpec { start: 0, duration: 3 }];
    let bytes = common::Spec { channels: 2, playtime: 3, tracks, audio: common::pack(&frames) }.build();
    let d = open(&bytes);
    let o = options();
    let mut l = ptr::null_mut();
    assert_eq!(
        c!(sb_output_layout_new(d, 0, 0, 3, SbOutputFormat::DsdiffDsd as u32, o, &mut l)),
        SbStatus::Ok
    );
    // 100 bytes of frame 1's sound data.
    let at = info(l).size - 4704 * 2 - 100;
    let (mut first, mut end) = (0, 0);
    assert_eq!(c!(sb_output_layout_positions(l, at, 100, &mut first, &mut end)), SbStatus::Ok);
    assert_eq!((first, end), (1, 2));
    // One allocation: the content at [0, n), then room for the buffer.
    let n = 2 * 4704;
    let mut arena = frame(1);
    arena.resize(n + 200, 0);
    let base = arena.as_mut_ptr();
    let position = |content: *const u8| SbPositionData {
        content,
        content_len: n,
        dst_equivalent: ptr::null(),
        dst_equivalent_len: 0,
        number: 1,
        reserved: 0,
    };
    let mut written = 0;
    // A buffer over the content's last byte, and one starting inside it: refused.
    for start in [n - 1, n - 100] {
        let p = [position(base)];
        let buf = c!(base.add(start));
        let s = c!(sb_output_layout_fill(l, at, buf, 100, p.as_ptr(), 1, &mut written));
        assert_eq!(s, SbStatus::InvalidArgument, "buffer at {start}");
    }
    // The DST equivalent is checked too.
    let p = [SbPositionData { dst_equivalent: base, dst_equivalent_len: n, ..position(c!(base.add(n))) }];
    assert_eq!(
        c!(sb_output_layout_fill(l, at, base.add(n - 1), 100, p.as_ptr(), 1, &mut written)),
        SbStatus::InvalidArgument
    );
    // A buffer over the positions array.
    let mut p = [position(base), position(base)];
    let over = p.as_mut_ptr().cast::<u8>();
    assert_eq!(
        c!(sb_output_layout_fill(l, at, over, 8, p.as_ptr(), 1, &mut written)),
        SbStatus::InvalidArgument
    );
    // Right after the content: accepted, and the bytes are the content's.
    let p = [position(base)];
    assert_eq!(c!(sb_output_layout_fill(l, at, base.add(n), 100, p.as_ptr(), 1, &mut written)), SbStatus::Ok);
    assert_eq!(written, 100);
    assert_eq!(arena[n..n + 100], frame(1)[n - 100..]);
    c!(sb_output_layout_free(l));
    c!(sb_output_options_free(o));
    c!(sb_disc_free(d));
}

/// A caller-supplied ID3v2.3 tag through `sb_output_options_set_raw_tag` (specification 8,
/// 8.2.6 item 4 and 8.6 row 3, sacd-spec e069569): the file ends with it unchanged and
/// equals the Rust API's; a header that fails a check is refused when the layout is made.
#[test]
#[cfg_attr(miri, ignore = "rayon's crossbeam-epoch fails Stacked Borrows; run with Tree Borrows")]
fn caller_id3v23_tag() {
    let frames: Vec<common::PFrame> = (0..2u32)
        .map(|number| common::PFrame {
            number,
            dst: false,
            data: (0..2 * 4704).map(|i| (i as u32 * 5 + number) as u8).collect(),
        })
        .collect();
    let tracks = vec![common::TrackSpec { start: 0, duration: 2 }];
    let bytes = common::Spec { channels: 2, playtime: 2, tracks, audio: common::pack(&frames) }.build();
    let d = open(&bytes);
    let mut disc = Disc::open(Cursor::new(bytes.clone())).unwrap();
    // A 300-byte body (size bytes 00 00 02 2C) that is no valid frame, and flag bit 7.
    let mut tag = b"ID3\x03\x00\x80\x00\x00\x02\x2C".to_vec();
    tag.extend((0..300u32).map(|i| (i * 11) as u8));
    let o = options();
    assert_eq!(c!(sb_output_options_set_raw_tag(o, tag.as_ptr(), tag.len())), SbStatus::Ok);
    let rust = OutputOptions::default().with_tags(Tags::Raw(tag.clone()));
    let layout = disc.output_layout(Slot::TwoChannel, 0, 2, Format::Dsf, &rust).unwrap();
    let want = rust_file(&mut disc, &layout);
    assert_eq!(want[want.len() - tag.len()..], tag[..]);
    let pointer = u64::from_le_bytes(want[20..28].try_into().unwrap());
    assert_eq!(pointer, (want.len() - tag.len()) as u64);
    let mut l = ptr::null_mut();
    assert_eq!(c!(sb_output_layout_new(d, 0, 0, 2, SbOutputFormat::Dsf as u32, o, &mut l)), SbStatus::Ok);
    assert_eq!(read(l, d, 7_000), want);
    c!(sb_output_layout_free(l));
    // A low flag bit, a size one short, version 2.3.1: refused.
    for (at, b) in [(5, 0x01), (9, 0x2B), (4, 0x01)] {
        let mut bad = tag.clone();
        bad[at] = b;
        assert_eq!(c!(sb_output_options_set_raw_tag(o, bad.as_ptr(), bad.len())), SbStatus::Ok);
        let mut l = ptr::null_mut();
        assert_eq!(
            c!(sb_output_layout_new(d, 0, 0, 2, SbOutputFormat::Dsf as u32, o, &mut l)),
            SbStatus::InvalidArgument
        );
        assert!(l.is_null());
    }
    c!(sb_output_options_free(o));
    c!(sb_disc_free(d));
}

/// Caller chunks (specification 8, 8.4.2 item 5, at sacd-spec `e069569`; 8.6 row 33)
/// added through the C ABI give the Rust API's file, size and regions; the region info
/// counts them.
#[test]
#[cfg_attr(miri, ignore = "rayon's crossbeam-epoch fails Stacked Borrows; run with Tree Borrows")]
fn caller_chunks_equal_the_rust_api() {
    let frames: Vec<common::PFrame> = (0..2u32)
        .map(|number| common::PFrame { number, dst: false, data: vec![number as u8; 2 * 4704] })
        .collect();
    let tracks = vec![common::TrackSpec { start: 0, duration: 2 }];
    let bytes = common::Spec { channels: 2, playtime: 2, tracks, audio: common::pack(&frames) }.build();
    let d = open(&bytes);
    let mut disc = Disc::open(Cursor::new(bytes.clone())).unwrap();
    let o = options();
    assert_eq!(c!(sb_output_options_add_chunk(o, b"ID3 ".as_ptr(), [1u8, 2, 3].as_ptr(), 3)), SbStatus::Ok);
    assert_eq!(c!(sb_output_options_add_chunk(o, b"ID3 ".as_ptr(), ptr::null(), 0)), SbStatus::Ok);
    let rust = OutputOptions::default().with_dsdiff_caller_chunks(vec![
        CallerChunk::new(*b"ID3 ", vec![1, 2, 3]).unwrap(),
        CallerChunk::new(*b"ID3 ", Vec::new()).unwrap(),
    ]);
    let layout = disc.output_layout(Slot::TwoChannel, 0, 2, Format::DsdiffDsd, &rust).unwrap();
    let want = rust_file(&mut disc, &layout);
    assert_eq!(want[want.len() - 28..], *b"ID3 \0\0\0\0\0\0\0\x03\x01\x02\x03\0ID3 \0\0\0\0\0\0\0\0");
    let mut l = ptr::null_mut();
    assert_eq!(
        c!(sb_output_layout_new(d, 0, 0, 2, SbOutputFormat::DsdiffDsd as u32, o, &mut l)),
        SbStatus::Ok
    );
    let i = info(l);
    assert_eq!(i.size, want.len() as u64);
    assert_eq!(i.regions as usize, layout.regions().len());
    let mut r = SbOutputRegion::default();
    assert_eq!(c!(sb_output_layout_region(l, i.regions - 1, &mut r)), SbStatus::Ok);
    assert_eq!((r.kind, r.offset + r.len), (SbOutputRegionKind::Trailer as u32, i.size));
    assert!(r.len >= 28);
    assert_eq!(read(l, d, 5_000), want);
    c!(sb_output_layout_free(l));
    // Cleared, the file is the default one.
    assert_eq!(c!(sb_output_options_clear_chunks(o)), SbStatus::Ok);
    assert_eq!(
        c!(sb_output_layout_new(d, 0, 0, 2, SbOutputFormat::DsdiffDsd as u32, o, &mut l)),
        SbStatus::Ok
    );
    assert_eq!(info(l).size, want.len() as u64 - 28);
    c!(sb_output_layout_free(l));
    c!(sb_output_options_free(o));
    c!(sb_disc_free(d));
}

/// Caller chunk IDs are checked when added (8.4.1 item 3, 8.4.2 item 5).
#[test]
fn caller_chunk_ids_are_checked() {
    let o = options();
    for id in
        [b"FRM8", b"FVER", b"PROP", b"DSD ", b"DST ", b"DSTI", b"COMT", b"DIIN", b"MANF", b" ID3", b"ID3\x7F"]
    {
        assert_eq!(
            c!(sb_output_options_add_chunk(o, id.as_ptr(), [0u8].as_ptr(), 1)),
            SbStatus::InvalidArgument
        );
    }
    assert_eq!(c!(sb_output_options_add_chunk(o, ptr::null(), ptr::null(), 0)), SbStatus::InvalidArgument);
    assert_eq!(
        c!(sb_output_options_add_chunk(o, b"ID3 ".as_ptr(), ptr::null(), 1)),
        SbStatus::InvalidArgument
    );
    assert_eq!(
        c!(sb_output_options_add_chunk(ptr::null(), b"ID3 ".as_ptr(), ptr::null(), 0)),
        SbStatus::InvalidArgument
    );
    assert_eq!(c!(sb_output_options_add_chunk(o, b"ID3 ".as_ptr(), [7u8; 5].as_ptr(), 5)), SbStatus::Ok);
    assert_eq!(c!(sb_output_options_clear_chunks(o)), SbStatus::Ok);
    assert_eq!(c!(sb_output_options_clear_chunks(ptr::null())), SbStatus::InvalidArgument);
    c!(sb_output_options_free(o));
}
