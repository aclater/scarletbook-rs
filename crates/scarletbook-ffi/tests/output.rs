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

use scarletbook::output::{
    DsfBitsPerSample, DstIndexOffset, DstLengths, Format, InvalidDst, Layout, OutputOptions, TagItem, TagSet,
    Tags,
};
use scarletbook::{Disc, Slot};
use scarletbook_ffi::*;

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
