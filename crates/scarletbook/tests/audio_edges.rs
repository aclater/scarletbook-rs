// SPDX-License-Identifier: MIT OR Apache-2.0

//! Edges of the audio cursors that the other suites leave out: the accessors of each cursor
//! and iterator, continuations that do not log, continuations after a failed read, images
//! shorter than a cursor expects, provisional starts from provisional checkpoints, checks of
//! saved state at the start sector itself, the cap on invalid-frame lines, and panics and
//! re-entry on the decoding pool. Each test checks what specification 4 (or the API
//! documentation, for behaviour the specification leaves to the reader) requires.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::cell::Cell;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use common::{PFrame, S, Spec, TrackSpec, pack};
use scarletbook::audio::{FRAME_BYTES_PER_CHANNEL, MAX_PROBLEM_LINES};
use scarletbook::log::{Level, Log};
use scarletbook::{
    AreaCursor, AreaIndex, AudioFrame, Disc, Error, FrameContent, FrameStatus, IndexOptions, Opening,
    ResumedFrom, SeekPolicy, Slot, Speculation, SpeculativeOptions, Stored, Suspect, TrackAudioCursor,
    TrackCursor, TrackFrame,
};

const SLOT: Slot = Slot::TwoChannel;

/// DST frames `0..n` of 4 000 to 10 000 bytes, in order: a well-formed area.
fn frames(n: u32) -> Vec<PFrame> {
    (0..n)
        .map(|k| PFrame { number: k, dst: true, data: vec![k as u8; 4000 + (k as usize * 37) % 6000] })
        .collect()
}

/// A 2-channel disc of `fs` (in disc order), one track over the play time `playtime`.
fn image(fs: &[PFrame], playtime: u32) -> Vec<u8> {
    Spec { channels: 2, playtime, tracks: vec![TrackSpec { start: 0, duration: playtime }], audio: pack(fs) }
        .build()
}

/// `img` without its last `n` sectors: the area TOC then places audio sectors past the end
/// of the image (specification 4, 4.2 items 1 and 3).
fn truncated(mut img: Vec<u8>, n: usize) -> Vec<u8> {
    img.truncate(img.len() - n * S);
    img
}

fn options() -> SpeculativeOptions {
    SpeculativeOptions::new().with_min_gap_sectors(64)
}

fn read<R: Read + Seek>(disc: &mut Disc<R>, cursor: TrackCursor) -> (Vec<TrackFrame>, TrackCursor) {
    let mut it = disc.read_frames(cursor);
    let v = it.by_ref().map(Result::unwrap).collect();
    (v, it.into_cursor())
}

fn decoded<R: Read + Seek>(disc: &mut Disc<R>, cursor: TrackAudioCursor) -> Vec<AudioFrame> {
    disc.read_track_audio(cursor).map(Result::unwrap).collect()
}

fn plain_frames(img: &[u8], first: u32, end: u32) -> Vec<TrackFrame> {
    let mut disc = Disc::open(Cursor::new(img.to_vec())).unwrap();
    let c = disc.range_frames_cursor(SLOT, first, end).unwrap();
    read(&mut disc, c).0
}

fn plain_audio(img: &[u8], first: u32, end: u32) -> Vec<AudioFrame> {
    let mut disc = Disc::open(Cursor::new(img.to_vec())).unwrap();
    let c = disc.range_audio_cursor(SLOT, first, end).unwrap();
    decoded(&mut disc, c)
}

type Lines = Arc<Mutex<Vec<(Level, String)>>>;

fn recorder(level: Level) -> (Log, Lines) {
    let lines = Lines::default();
    let sink = Arc::clone(&lines);
    (Log::new(level, move |l, m| sink.lock().unwrap().push((l, m.to_string()))), lines)
}

/// The DST equivalent of a missing position (specification 4, 4.6.1 item 3): the byte
/// 0x00, then `C x 4704` bytes of 0x69.
fn missing_equivalent(channels: u8) -> Vec<u8> {
    let mut v = vec![0x69; 1 + usize::from(channels) * FRAME_BYTES_PER_CHANNEL];
    v[0] = 0;
    v
}

/// A provisional reading through the borrowing iterator reports itself as one from its
/// first frame on (4.2 item 8: positions produced this way MUST be reported as
/// provisional), names its reading as the verdict's event does, and its counts begin at
/// its `L`; the iterator and the cursor it hands back agree, and the range still to come
/// shrinks as positions are returned (4.7.1).
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn track_frames_report_a_provisional_reading() {
    let img = image(&frames(1000), 1000);
    let want = plain_frames(&img, 500, 520);
    let mut disc = Disc::open(Cursor::new(img)).unwrap();
    disc.enable_seek_index(SLOT, IndexOptions::new()).unwrap();
    disc.set_seek_policy(SLOT, SeekPolicy::Speculative(options())).unwrap();
    let mut it = disc.range_frames(SLOT, 500, 520).unwrap();
    assert_eq!(it.range(), (500, 520));
    assert!(
        matches!(it.speculation(), Speculation::Pending { lsn: 0, first: 500, .. }),
        "chosen on the first read"
    );
    let first = it.next().unwrap().unwrap();
    assert_eq!(first, want[0]);
    assert_eq!(it.range(), (501, 520));
    let Speculation::Pending { lsn, first: 500, .. } = it.speculation() else {
        panic!("{:?}", it.speculation())
    };
    assert!(lsn > 0);
    assert_eq!(it.claim_lsn(), Some(lsn));
    assert_eq!(it.suspect(), None, "a well-formed area read in increasing order");
    let id = it.claim().expect("a provisional reading is numbered");
    #[allow(deprecated)]
    let old_names = (it.counts_from(), it.speculation_id());
    assert_eq!(old_names, (Some(lsn), Some(id)));
    let (read_so_far, _) = it.progress().get();
    assert!(read_so_far > 0);
    assert_eq!(&*it.dst_equivalent(&FrameContent::Missing), missing_equivalent(2).as_slice());
    let rest: Vec<TrackFrame> = it.by_ref().map(Result::unwrap).collect();
    assert_eq!(rest, want[1..]);
    assert_eq!(it.range(), (520, 520));
    let c = it.into_cursor();
    assert_eq!((c.slot(), c.claim(), c.claim_lsn()), (SLOT, Some(id), Some(lsn)));
    assert_eq!(c.range(), (520, 520));
    // Its verdict names the same reading (4.2 item 8: confirmed on a well-formed area).
    while !disc.verify_speculation(SLOT, 1 << 20).unwrap() {}
    let events = disc.take_seek_events(SLOT);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].claim, id);
    assert!(matches!(events[0].speculation, Speculation::Confirmed { .. }));

    // Exact positions carry no number and count from the area's first sector.
    let mut exact = Disc::open(Cursor::new(image(&frames(40), 40))).unwrap();
    let mut it = exact.range_frames(SLOT, 10, 12).unwrap();
    it.next().unwrap().unwrap();
    assert_eq!((it.speculation(), it.claim(), it.claim_lsn()), (Speculation::Exact, None, None));
}

/// Decoded audio of an area whose TOC places audio sectors past the end of the image: the
/// frames those sectors would carry are missing, not an error (4.2 item 3); a provisional
/// reading of it is marked suspect for that reason (4.2 item 8 names it as an example), on
/// the iterator and on the cursor alike, and its missing and invalid counts are those of
/// its positions (4.7.1 item 6).
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn track_audio_reports_a_suspect_reading() {
    let img = truncated(image(&frames(1000), 1000), 40);
    let mut disc = Disc::open(Cursor::new(img.clone())).unwrap();
    let area = disc.toc().areas[0].clone();
    assert!(u64::from(area.audio_end_lsn) >= disc.toc().sectors, "audio past the end of the image");
    let want = plain_audio(&img, 990, 1000);
    assert!(want.iter().any(|f| f.status == FrameStatus::Missing), "frames of the missing sectors");
    disc.set_seek_policy(SLOT, SeekPolicy::Speculative(options())).unwrap();
    let c = disc.range_audio_cursor(SLOT, 990, 1000).unwrap();
    assert_eq!(c.slot(), SLOT);
    let mut it = disc.read_track_audio(c);
    let got: Vec<AudioFrame> = it.by_ref().map(Result::unwrap).collect();
    assert_eq!(got, want);
    let Speculation::Pending { lsn, first: 990, .. } = it.speculation() else {
        panic!("{:?}", it.speculation())
    };
    assert_eq!(it.suspect(), Some(Suspect::AreaLayout));
    assert_eq!(it.claim_lsn(), Some(lsn));
    let id = it.claim().unwrap();
    let missing = want.iter().filter(|f| f.status == FrameStatus::Missing).count() as u64;
    let invalid = want.iter().filter(|f| matches!(f.status, FrameStatus::Invalid(_))).count() as u64;
    assert_eq!((it.missing_frames(), it.invalid_dst_frames()), (missing, invalid));
    let c = it.into_cursor();
    assert_eq!((c.claim(), c.claim_lsn(), c.suspect()), (Some(id), Some(lsn), Some(Suspect::AreaLayout)));
    assert_eq!((c.missing_frames(), c.invalid_dst_frames()), (missing, invalid));

    // Without a disc, a cursor checks the end of the image itself.
    let n = disc.toc().sectors;
    let c = TrackAudioCursor::for_range_speculative(&area, 990, 1000, n, None, options()).unwrap();
    assert_eq!(decoded(&mut disc, c), want);
    let mut c = TrackAudioCursor::for_range_speculative(&area, 990, 1000, n, None, options()).unwrap();
    let mut got = Vec::new();
    while let Some(f) = c.next_frame(disc.image_mut()) {
        got.push(f.unwrap());
    }
    assert_eq!(got, want);
    assert_eq!(c.suspect(), Some(Suspect::AreaLayout));
    // A range outside the play range is a caller error (4.7.1 item 3).
    assert!(matches!(
        TrackAudioCursor::for_range_speculative(&area, 990, 1001, n, None, options()),
        Err(Error::FrameRangeInvalid { first: 990, end: 1001, playtime: 1000, .. })
    ));
}

/// A cursor made for an image of more sectors than the one it reads must not take the
/// missing sectors for the end of the area: the image is not the one it was checked against
/// (4.2 item 1 defines `S1'` from that image's size), so the read is refused, logged, and
/// the cursor ends. Made for the image it reads, the same range gives missing frames.
#[test]
#[cfg_attr(miri, ignore = "reads hundreds of sectors")]
fn a_cursor_for_a_longer_image_is_refused() {
    let full = image(&frames(40), 40);
    let short = truncated(full.clone(), 30);
    let long_disc = Disc::open(Cursor::new(full)).unwrap();
    let area = long_disc.toc().areas[0].clone();
    let n = long_disc.toc().sectors;
    let (log, lines) = recorder(Level::Error);
    let mut disc = Disc::options().with_log(log).open(Cursor::new(short.clone())).unwrap();
    assert!(disc.toc().sectors < n);

    // The positions of the sectors read before the missing ones, then the refusal, then
    // the end.
    let mut c = TrackCursor::for_range(&area, 0, 40, n).unwrap();
    let results: Vec<_> = std::iter::from_fn(|| c.next_frame(disc.image_mut())).collect();
    let (last, before) = results.split_last().unwrap();
    assert!(matches!(last, Err(Error::InvalidArgument(_))), "{last:?}");
    assert!(before.len() < 40 && before.iter().all(Result::is_ok));
    assert_eq!(
        before.iter().map(|f| f.as_ref().unwrap().clone()).collect::<Vec<_>>(),
        plain_frames(&short, 0, before.len() as u32)
    );
    assert!(
        lines.lock().unwrap().iter().any(|(l, m)| *l == Level::Error && m.contains("outside the image")),
        "{:?}",
        lines.lock().unwrap()
    );
    // The stored frame list's cursor: the frames of the sectors read before the missing
    // ones, then the refusal, then the end.
    let mut c = AreaCursor::new(&area, n);
    assert_eq!(c.resumed_from(), ResumedFrom::Start);
    let results: Vec<_> = std::iter::from_fn(|| c.next_frame(disc.image_mut())).collect();
    let (last, before) = results.split_last().unwrap();
    assert!(matches!(last, Err(Error::InvalidArgument(_))), "{last:?}");
    assert!(before.iter().all(Result::is_ok));

    // Sized for the image it reads: missing frames (4.2 item 3), no error.
    let got = plain_frames(&short, 0, 40);
    assert_eq!(got.len(), 40);
    assert!(matches!(got.last().unwrap().content, FrameContent::Missing));
    assert!(matches!(got[0].content, FrameContent::Dst(_)));
}

/// The unlogged forms of a continuation call no sink, and the cursors they open give
/// exactly the positions of a plain read: each pass gives the same frames (4.2 items 6 and
/// 7), whether it continues a scan, starts afresh or starts from a seek index.
#[test]
#[cfg_attr(miri, ignore = "reads hundreds of sectors")]
fn unlogged_continuations_open_alike() {
    let img = image(&frames(120), 120);
    let want = plain_frames(&img, 60, 80);
    let want_audio = plain_audio(&img, 60, 80);
    let (log, lines) = recorder(Level::Trace);
    let mut disc = Disc::options().with_log(log).open(Cursor::new(img.clone())).unwrap();
    disc.enable_seek_index(SLOT, IndexOptions::new().with_interval(8)).unwrap();

    // Frames, continued forward from a cursor that read 0..20.
    let c = disc.range_frames_cursor(SLOT, 0, 20).unwrap();
    let (_, prev) = read(&mut disc, c);
    let cont = disc.range_continuation(SLOT, 60, 80).unwrap();
    assert_eq!(cont.range(), (60, 80));
    let before = lines.lock().unwrap().len();
    let next = cont.frames_unlogged(prev);
    assert_eq!(lines.lock().unwrap().len(), before, "no line logged");
    assert_eq!(read(&mut disc, next).0, want);

    // Decoded audio, likewise.
    let c = disc.range_audio_cursor(SLOT, 0, 20).unwrap();
    let mut it = disc.read_track_audio(c);
    it.by_ref().for_each(|f| drop(f.unwrap()));
    let prev = it.into_cursor();
    let cont = disc.range_continuation(SLOT, 60, 80).unwrap();
    let before = lines.lock().unwrap().len();
    let next = cont.audio_unlogged(prev);
    assert_eq!(lines.lock().unwrap().len(), before, "no line logged");
    assert_eq!(decoded(&mut disc, next), want_audio);

    // A fresh cursor from the disc's index, which then takes what it records.
    let index_before = disc.seek_index(SLOT).unwrap().frontier();
    let cont = disc.range_continuation(SLOT, 100, 120).unwrap();
    let before = lines.lock().unwrap().len();
    let next = cont.fresh_frames();
    assert_eq!(lines.lock().unwrap().len(), before, "no line logged");
    assert_eq!(read(&mut disc, next).0, plain_frames(&img, 100, 120));
    assert!(disc.seek_index(SLOT).unwrap().frontier() > index_before, "the disc's index grew");

    // With the caller's own index: what the cursor recorded joins it, and the next range
    // starts from it.
    let area = disc.toc().areas[0].clone();
    let n = disc.toc().sectors;
    let mut index = AreaIndex::new(&area, n, IndexOptions::new().with_interval(8)).unwrap();
    let mut prev = TrackCursor::for_range_indexed(&area, 0, 50, n, &index).unwrap();
    while let Some(f) = prev.next_frame(disc.image_mut()) {
        f.unwrap();
    }
    assert_eq!(index.checkpoint_count(), 0);
    let cont = disc.range_continuation(SLOT, 10, 30).unwrap();
    assert_eq!(cont.log_frames_indexed(&prev, &index), Opening::FromIndex, "a range behind the cursor");
    let before = lines.lock().unwrap().len();
    let mut next = cont.frames_indexed_unlogged(prev, &mut index);
    assert_eq!(lines.lock().unwrap().len(), before, "no line logged");
    assert!(index.checkpoint_count() > 0, "the earlier cursor's checkpoints joined the index");
    assert!(matches!(next.resumed_from(), ResumedFrom::Checkpoint { .. }), "{:?}", next.resumed_from());
    let mut got = Vec::new();
    while let Some(f) = next.next_frame(disc.image_mut()) {
        got.push(f.unwrap());
    }
    assert_eq!(got, plain_frames(&img, 10, 30));
}

/// Cursors made without a disc continue over tracks and ranges in any order, each giving
/// the positions of a fresh cursor (4.2 item 6: every pass gives the same frames); a range
/// they cannot continue into is refused as 4.7.1 items 3 and 4 say.
#[test]
#[cfg_attr(miri, ignore = "reads hundreds of sectors")]
fn cursors_continue_over_tracks_and_ranges() {
    let fs = frames(60);
    let tracks = vec![TrackSpec { start: 0, duration: 20 }, TrackSpec { start: 20, duration: 40 }];
    let img = Spec { channels: 2, playtime: 60, tracks, audio: pack(&fs) }.build();
    let mut disc = Disc::open(Cursor::new(img.clone())).unwrap();
    let area = disc.toc().areas[0].clone();
    let n = disc.toc().sectors;
    let (t1, t2) = (area.tracks[0].clone(), area.tracks[1].clone());
    let drain = |c: &mut TrackCursor, disc: &mut Disc<Cursor<Vec<u8>>>| {
        std::iter::from_fn(|| c.next_frame(disc.image_mut())).map(Result::unwrap).collect::<Vec<_>>()
    };

    let mut c = TrackCursor::new(&area, &t1, n).unwrap();
    assert_eq!(drain(&mut c, &mut disc), plain_frames(&img, 0, 20));
    // Forward to track 2: the state is reused, so the scan still starts at the first sector.
    let mut c = c.continue_to(&area, &t2, n).unwrap();
    assert_eq!(c.range(), (20, 60));
    assert_eq!(drain(&mut c, &mut disc), plain_frames(&img, 20, 60));
    assert_eq!(c.resumed_from(), ResumedFrom::Start);
    // Backwards: a new pass.
    let mut c = c.continue_to_range(&area, 5, 15, n).unwrap();
    assert_eq!((c.range(), c.missing_frames()), ((5, 15), 0));
    assert_eq!(drain(&mut c, &mut disc), plain_frames(&img, 5, 15));
    assert!(matches!(
        c.continue_to_range(&area, 15, 5, n),
        Err(Error::FrameRangeInvalid { first: 15, end: 5, playtime: 60, .. })
    ));

    // Decoded audio, likewise; an empty range is skipped as `track-empty` (4.7.1 item 4).
    let c = TrackAudioCursor::for_range(&area, 10, 30, n).unwrap();
    let mut it = disc.read_track_audio(c);
    let got: Vec<AudioFrame> = it.by_ref().map(Result::unwrap).collect();
    assert_eq!(got, plain_audio(&img, 10, 30));
    let c = it.into_cursor();
    let c = c.continue_to_range(&area, 40, 50, n).unwrap();
    assert_eq!(decoded(&mut disc, c), plain_audio(&img, 40, 50));
    let c = TrackAudioCursor::for_range(&area, 0, 1, n).unwrap();
    assert!(matches!(c.continue_to_range(&area, 7, 7, n), Err(Error::TrackEmpty)));
    assert!(matches!(TrackAudioCursor::for_range(&area, 0, 61, n), Err(Error::FrameRangeInvalid { .. })));
}

/// A plain frame, a DST frame and a missing position (specification 4, 4.6.1 item 1),
/// read with their stored content: [`Stored::bytes`] is exactly the stored bytes, the
/// plain frame's being its decoded data, and none for a missing position.
#[test]
fn stored_bytes_of_each_kind() {
    let plain =
        |k: u32| PFrame { number: k, dst: false, data: vec![k as u8 + 1; 2 * FRAME_BYTES_PER_CHANNEL] };
    let fs = vec![plain(0), PFrame { number: 1, dst: true, data: vec![0xA5; 300] }, plain(3)];
    let img = image(&fs, 4);
    let want = plain_frames(&img, 0, 4);
    let mut disc = Disc::open(Cursor::new(img)).unwrap();
    let mut c = disc.range_audio_cursor(SLOT, 0, 4).unwrap();
    c.keep_stored().unwrap();
    let mut kinds = Vec::new();
    for w in &want {
        let (frame, stored) = c.next_frame_with_stored(disc.image_mut()).unwrap().unwrap();
        let bytes: &[u8] = match &w.content {
            FrameContent::Dst(b) | FrameContent::Dsd(b) => b,
            FrameContent::Missing => &[],
        };
        assert_eq!(stored.bytes(&frame), bytes, "frame {}", w.number);
        assert_eq!(stored.content(&frame), w.content);
        kinds.push(stored.clone());
        if matches!(stored, Stored::Dsd) {
            assert_eq!(frame.status, FrameStatus::Dsd);
        }
    }
    assert!(matches!(kinds[..], [Stored::Dsd, Stored::Dst(_), Stored::Missing, Stored::Dsd]), "{kinds:?}");
    assert!(c.next_frame_with_stored(disc.image_mut()).is_none());
}

/// A reader that fails every read touching byte `at` while `on` is set.
struct Failing {
    inner: Cursor<Vec<u8>>,
    at: u64,
    on: Rc<Cell<bool>>,
}

impl Read for Failing {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let pos = self.inner.position();
        if self.on.get() && (pos..pos + buf.len() as u64).contains(&self.at) {
            return Err(std::io::Error::other("injected"));
        }
        self.inner.read(buf)
    }
}

impl Seek for Failing {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

/// DST frames `0..n` of 20 000 bytes: about 10 sectors each, so that the cursor's first
/// read (32 sectors) holds frames 0 to 2 and frame 3 needs a second one.
fn large_frames(n: u32) -> Vec<PFrame> {
    (0..n).map(|k| PFrame { number: k, dst: true, data: vec![k as u8; 20_000] }).collect()
}

/// A decoded read that meets a read error returns the positions before it, then the error,
/// also with its stored content. Continued before the error is returned, it is not reused
/// (its pass did not complete, so it cannot stand for one, 4.2 item 6): the continuation
/// opens a new pass, or from the disc's index, and gives exactly the range's positions.
#[test]
#[cfg_attr(miri, ignore = "reads hundreds of sectors")]
fn a_failed_read_is_not_continued() {
    let img = image(&large_frames(12), 12);
    let first_audio = common::AUDIO_LSN as u64;
    let at = (first_audio + 40) * S as u64;
    for with_index in [false, true] {
        let on = Rc::new(Cell::new(true));
        let mut disc =
            Disc::open(Failing { inner: Cursor::new(img.clone()), at, on: Rc::clone(&on) }).unwrap();
        if with_index {
            disc.enable_seek_index(SLOT, IndexOptions::new()).unwrap();
        }
        let mut c = disc.range_audio_cursor(SLOT, 0, 6).unwrap();
        #[cfg(feature = "parallel")]
        c.set_decode_on(scarletbook::DecodeOn::CallingThread);
        c.keep_stored().unwrap();
        // Frames 0 to 2 come from the first read; reading on for frame 3 fails, and the
        // error waits behind frame 2.
        for k in 0..3 {
            let (f, stored) = c.next_frame_with_stored(disc.image_mut()).unwrap().unwrap();
            assert_eq!(f.number, k);
            assert!(matches!(stored, Stored::Dst(ref b) if b.len() == 20_000));
        }
        let opening = disc.range_continuation(SLOT, 8, 12).unwrap().log_audio(&c);
        assert_eq!(
            opening,
            if with_index { Opening::FromIndex } else { Opening::Fresh },
            "index {with_index}"
        );
        // The error itself comes next, as a cursor read the same way shows.
        let area = disc.toc().areas[0].clone();
        let mut probe = TrackAudioCursor::for_range(&area, 0, 6, disc.toc().sectors).unwrap();
        #[cfg(feature = "parallel")]
        probe.set_decode_on(scarletbook::DecodeOn::CallingThread);
        probe.keep_stored().unwrap();
        let mut results = Vec::new();
        while let Some(r) = probe.next_frame_with_stored(disc.image_mut()) {
            results.push(r.map(|(f, _)| f.number));
        }
        assert!(matches!(results[..], [Ok(0), Ok(1), Ok(2), Err(Error::Io(_))]), "{results:?}");
        on.set(false);
        let next = disc.range_continuation(SLOT, 8, 12).unwrap().audio_as(c, opening);
        assert_eq!(decoded(&mut disc, next), plain_audio(&img, 8, 12), "index {with_index}");
    }
}

/// Without seek hints, a later provisional start near an earlier provisional reading
/// begins at one of that reading's provisional checkpoints, with no search (4.2 item 8
/// lets the reader choose `L` as it likes), and gives the positions of an exact read on a
/// well-formed area; in an area whose TOC places audio past the end of the image, such a
/// reading is suspect as well. Each is confirmed once checked (4.2 item 8, informative:
/// (a) and (b) hold on a well-formed area).
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn provisional_checkpoints_serve_without_hints() {
    for cut in [0, 40] {
        let img = truncated(image(&frames(1000), 1000), cut);
        let mut disc = Disc::open(Cursor::new(img.clone())).unwrap();
        disc.enable_seek_index(SLOT, IndexOptions::new().with_interval(16).with_hints(0, 0)).unwrap();
        disc.set_seek_policy(SLOT, SeekPolicy::Speculative(options())).unwrap();
        let c = disc.range_frames_cursor(SLOT, 500, 700).unwrap();
        let (_, c) = read(&mut disc, c);
        assert!(matches!(c.resumed_from(), ResumedFrom::Speculative { probes, .. } if probes > 0));
        assert_eq!(disc.seek_index(SLOT).unwrap().hint_count(), 0);
        let suspect = (cut > 0).then_some(Suspect::AreaLayout);
        for first in [560, 620, 690] {
            let c = disc.range_frames_cursor(SLOT, first, first + 5).unwrap();
            let (got, c) = read(&mut disc, c);
            let ResumedFrom::Speculative { lsn, probes: 0, .. } = c.resumed_from() else {
                panic!("cut {cut} first {first}: {:?}", c.resumed_from())
            };
            assert!(
                matches!(c.speculation(), Speculation::Pending { lsn: l, first: f, .. } if l == lsn && f == first)
            );
            assert_eq!(c.suspect(), suspect, "cut {cut} first {first}");
            assert_eq!(got, plain_frames(&img, first, first + 5), "cut {cut} first {first}");
        }
        while !disc.verify_speculation(SLOT, 1 << 20).unwrap() {}
        let events = disc.take_seek_events(SLOT);
        assert_eq!(events.len(), 4);
        assert!(events.iter().all(|e| matches!(e.speculation, Speculation::Confirmed { .. })), "{events:?}");
    }
}

/// Another pressing: the same frames (numbers, kinds, sizes), other bytes.
fn other_pressing(n: u32) -> Vec<PFrame> {
    (0..n)
        .map(|k| PFrame { number: k, dst: true, data: vec![k as u8 ^ 0x5A; 4000 + (k as usize * 37) % 6000] })
        .collect()
}

/// Saved state establishes (a) and (b) only for an image whose sectors are the bytes the
/// pass processed (4.2 item 9). A provisional reading that begins exactly at a checkpoint
/// of an index made from another pressing (same layout, other bytes) is therefore never
/// confirmed from it, although no sector before its `L` needs reading: the checkpoint's
/// own sector differs. Every reading stays unverified.
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn a_foreign_checkpoint_at_l_does_not_confirm() {
    let mut a = Disc::open(Cursor::new(image(&other_pressing(1000), 1000))).unwrap();
    let mut b = Disc::open(Cursor::new(image(&frames(1000), 1000))).unwrap();
    let area = b.toc().areas[0].clone();
    let n = b.toc().sectors;
    assert_eq!((area.clone(), n), (a.toc().areas[0].clone(), a.toc().sectors), "same key");
    let opts = IndexOptions::new().with_interval(16).with_hints(0, 0);
    b.enable_seek_index(SLOT, opts).unwrap();
    b.set_seek_policy(SLOT, SeekPolicy::Speculative(options())).unwrap();
    let c = b.range_frames_cursor(SLOT, 500, 700).unwrap();
    read(&mut b, c);
    // A start at a provisional checkpoint: `L` lies on the checkpoint grid.
    let c = b.range_frames_cursor(SLOT, 620, 625).unwrap();
    let (_, c) = read(&mut b, c);
    let ResumedFrom::Speculative { lsn, probes: 0, .. } = c.resumed_from() else {
        panic!("{:?}", c.resumed_from())
    };
    assert_eq!((lsn - u64::from(area.audio_start_lsn)) % 16, 0);
    // A's complete index, saved and loaded (so of no image in particular), merged in.
    a.enable_seek_index(SLOT, opts).unwrap();
    while !a.extend_seek_index(SLOT, 1 << 20).unwrap() {}
    let bytes = a.seek_index(SLOT).unwrap().to_bytes();
    let loaded = AreaIndex::from_bytes(&area, n, &bytes, usize::MAX).unwrap();
    let mut kept = b.take_seek_index(SLOT).unwrap();
    assert_eq!(kept.pending_claim_count(), 2);
    kept.merge(&loaded).unwrap();
    b.set_seek_index(SLOT, kept).unwrap();
    while !b.verify_speculation(SLOT, 1 << 20).unwrap() {}
    assert!(matches!(c.speculation(), Speculation::Unverified { .. }), "{:?}", c.speculation());
    let events = b.take_seek_events(SLOT);
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|e| matches!(e.speculation, Speculation::Unverified { .. })), "{events:?}");
}

/// Invalid DST frames are each filled with 0x55 and counted (specification 4, 4.6;
/// 4.7.1 item 6), however many there are; their log lines are capped per cursor, with one
/// line saying the rest are suppressed (crate::log).
#[test]
#[cfg_attr(miri, ignore = "decodes thousands of frames")]
fn invalid_frames_are_counted_past_the_line_cap() {
    let n = (MAX_PROBLEM_LINES + 20) as u32;
    // A DST frame of the single byte 0x00 says "not DST coded" (specification 5) but holds
    // no audio: invalid.
    let fs: Vec<PFrame> = (0..n).map(|k| PFrame { number: k, dst: true, data: vec![0] }).collect();
    let img = image(&fs, n);
    let (log, lines) = recorder(Level::Warn);
    let mut disc = Disc::options().with_log(log).open(Cursor::new(img)).unwrap();
    let mut it = disc.range_audio(SLOT, 0, n).unwrap();
    let mut count = 0;
    for f in it.by_ref() {
        let f = f.unwrap();
        assert!(matches!(f.status, FrameStatus::Invalid(_)), "frame {}: {:?}", f.number, f.status);
        assert!(f.data.iter().all(|&b| b == 0x55));
        count += 1;
    }
    assert_eq!(count, n);
    assert_eq!((it.invalid_dst_frames(), it.missing_frames()), (u64::from(n), 0));
    let lines = lines.lock().unwrap();
    let invalid = lines.iter().filter(|(_, m)| m.contains(": invalid DST frame (")).count();
    assert_eq!(invalid, MAX_PROBLEM_LINES);
    let suppressed = format!(
        "area 2ch: further invalid-DST-frame lines suppressed (at most {MAX_PROBLEM_LINES} per cursor)"
    );
    assert_eq!(lines.iter().filter(|(_, m)| *m == suppressed).count(), 1, "{:?}", &lines[lines.len() - 3..]);
}

/// Checkpoints recorded by a cursor read without the disc's iterators are taken into the
/// disc's index on request, from either kind of track cursor (crate::Disc docs); a cursor
/// made without an index has recorded none, and taking them changes nothing.
#[test]
#[cfg_attr(miri, ignore = "reads hundreds of sectors")]
fn checkpoints_are_absorbed_from_every_cursor() {
    let img = image(&frames(200), 200);
    let mut disc = Disc::open(Cursor::new(img)).unwrap();
    disc.enable_seek_index(SLOT, IndexOptions::new().with_interval(8)).unwrap();
    let frontier = |disc: &Disc<Cursor<Vec<u8>>>| disc.seek_index(SLOT).unwrap().frontier();
    let start = frontier(&disc);

    let mut c = disc.range_frames_cursor(SLOT, 0, 50).unwrap();
    while let Some(f) = c.next_frame(disc.image_mut()) {
        f.unwrap();
    }
    assert_eq!(frontier(&disc), start, "not taken yet");
    disc.absorb_seek_checkpoints(&mut c);
    let after_frames = frontier(&disc);
    assert!(after_frames > start);

    let mut c = disc.range_audio_cursor(SLOT, 0, 150).unwrap();
    while let Some(f) = c.next_frame(disc.image_mut()) {
        f.unwrap();
    }
    disc.absorb_seek_checkpoints(&mut c);
    assert!(frontier(&disc) > after_frames);

    let area = disc.toc().areas[0].clone();
    let mut c = AreaCursor::new(&area, disc.toc().sectors);
    while let Some(f) = c.next_frame(disc.image_mut()) {
        f.unwrap();
    }
    let before = frontier(&disc);
    disc.absorb_seek_checkpoints(&mut c);
    assert_eq!(frontier(&disc), before);
    let mut index = AreaIndex::new(&area, disc.toc().sectors, IndexOptions::new()).unwrap();
    index.absorb(&mut c).unwrap();
    assert_eq!(index.checkpoint_count(), 0);
}

/// The message of a caught panic.
#[cfg(feature = "parallel")]
fn message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_default()
}

/// A sink that panics on a pool thread, while a job decodes DST frames there, and again on
/// the line that reports it: the job's panic comes back on the caller, from the call that
/// takes the job's first frame, and the call that takes its other frame panics too
/// (crate::log: "the panic is caught with the decoding job's and comes back on the caller
/// as a panic while decoding"). The frames of the other jobs are those of a plain read.
#[cfg(feature = "parallel")]
#[test]
#[cfg_attr(miri, ignore = "starts threads")]
fn a_panic_on_the_pool_comes_back_on_the_caller() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let img = image(&frames(40), 40);
    let want = plain_audio(&img, 0, 40);
    let fired = Arc::new(AtomicBool::new(false));
    let once = Arc::clone(&fired);
    let log = Log::new(Level::Trace, move |_, m| {
        if rayon::current_thread_index().is_none() {
            return;
        }
        let m = m.to_string();
        if (m.starts_with("decoding DST frames") && !once.swap(true, Ordering::SeqCst))
            || m.starts_with("panic while decoding")
        {
            panic!("sink on the pool");
        }
    });
    let mut disc = Disc::options().with_log(log).open(Cursor::new(img)).unwrap();
    let pool = Arc::new(rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap());
    let mut c = disc.range_audio_cursor(SLOT, 0, 40).unwrap();
    c.set_decode_on(scarletbook::DecodeOn::Pool(pool));
    let mut outcomes = Vec::new();
    loop {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| c.next_frame(disc.image_mut()))) {
            Ok(None) => break,
            Ok(Some(f)) => outcomes.push(Ok(f.unwrap())),
            Err(payload) => outcomes.push(Err(message(&*payload))),
        }
    }
    assert!(fired.load(Ordering::SeqCst));
    let panics: Vec<_> =
        outcomes.iter().enumerate().filter_map(|(i, o)| o.as_ref().err().map(|m| (i, m))).collect();
    assert_eq!(panics.len(), 2, "{panics:?}");
    assert_eq!(panics[0].1, "sink on the pool", "the job's own panic, not the second one");
    assert_eq!(panics[1].1, "a group of DST frames panicked while decoding");
    assert_eq!(panics[1].0, panics[0].0 + 1, "the two frames of one job");
    for f in outcomes.iter().filter_map(|o| o.as_ref().ok()) {
        assert_eq!(f, &want[f.number as usize]);
    }
}

/// A sink that, on a pool thread in the middle of a job, decodes another range on that same
/// pool (crate::log allows it: the thread runs pending jobs while it waits, entering the
/// decoding again): both reads give exactly the frames of a plain read, whichever thread
/// decodes them (crate::DecodeOn).
#[cfg(feature = "parallel")]
#[test]
#[cfg_attr(miri, ignore = "starts threads")]
fn a_sink_may_decode_on_the_same_pool() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let img = image(&frames(60), 60);
    let want = plain_audio(&img, 0, 60);
    let inner = Arc::new(Mutex::new(Some(Disc::open(Cursor::new(img.clone())).unwrap())));
    let nested: Arc<Mutex<Vec<AudioFrame>>> = Arc::default();
    let fired = Arc::new(AtomicBool::new(false));
    let (once, disc_in, out) = (Arc::clone(&fired), Arc::clone(&inner), Arc::clone(&nested));
    let log = Log::new(Level::Trace, move |_, m| {
        if rayon::current_thread_index().is_none()
            || !m.to_string().starts_with("decoding DST frames")
            || once.swap(true, Ordering::SeqCst)
        {
            return;
        }
        let mut disc = disc_in.lock().unwrap().take().unwrap();
        // Decoded on the current pool (the default): the one this job runs on.
        let c = disc.range_audio_cursor(SLOT, 0, 60).unwrap();
        *out.lock().unwrap() = decoded(&mut disc, c);
    });
    let mut disc = Disc::options().with_log(log).open(Cursor::new(img)).unwrap();
    let pool = Arc::new(rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap());
    let mut c = disc.range_audio_cursor(SLOT, 0, 60).unwrap();
    c.set_decode_on(scarletbook::DecodeOn::Pool(pool));
    assert_eq!(decoded(&mut disc, c), want);
    assert!(fired.load(Ordering::SeqCst));
    assert_eq!(*nested.lock().unwrap(), want);
}

/// A reader that requests cancellation right after its next read, once armed.
struct Cancelling {
    inner: Cursor<Vec<u8>>,
    token: Rc<std::cell::OnceCell<scarletbook::CancelToken>>,
    armed: Rc<Cell<bool>>,
}

impl Read for Cancelling {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if self.armed.replace(false) {
            self.token.get().unwrap().cancel();
        }
        Ok(n)
    }
}

impl Seek for Cancelling {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

/// A provisional start whose search is cancelled after its first probe returns
/// [`Error::Cancelled`]; once the request is withdrawn the search runs again from the
/// start, choosing the same `L` after the same probes as an uncancelled one, and the
/// positions are those of a plain read (4.2 item 8 on a well-formed area).
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn a_cancelled_search_starts_again() {
    let img = image(&frames(1000), 1000);
    let want = plain_frames(&img, 700, 710);
    let mut quiet = Disc::open(Cursor::new(img.clone())).unwrap();
    quiet.set_seek_policy(SLOT, SeekPolicy::Speculative(options())).unwrap();
    let c = quiet.range_frames_cursor(SLOT, 700, 710).unwrap();
    let (_, c) = read(&mut quiet, c);
    let uncancelled = c.resumed_from();
    let ResumedFrom::Speculative { probes, .. } = uncancelled else { panic!("{uncancelled:?}") };
    assert!(probes >= 2, "the cancellation lands between two probes");

    let (token, armed) = (Rc::new(std::cell::OnceCell::new()), Rc::new(Cell::new(false)));
    let reader = Cancelling { inner: Cursor::new(img), token: Rc::clone(&token), armed: Rc::clone(&armed) };
    let mut disc = Disc::open(reader).unwrap();
    token.set(disc.cancel_token()).unwrap();
    disc.set_seek_policy(SLOT, SeekPolicy::Speculative(options())).unwrap();
    let c = disc.range_frames_cursor(SLOT, 700, 710).unwrap();
    let mut it = disc.read_frames(c);
    armed.set(true);
    assert!(matches!(it.next(), Some(Err(Error::Cancelled))));
    assert!(matches!(it.speculation(), Speculation::Pending { lsn: 0, .. }), "nothing chosen yet");
    token.get().unwrap().withdraw();
    let got: Vec<TrackFrame> = it.by_ref().map(Result::unwrap).collect();
    assert_eq!(got, want);
    assert_eq!(it.into_cursor().resumed_from(), uncancelled);
}
