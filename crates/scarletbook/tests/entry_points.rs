// SPDX-License-Identifier: MIT OR Apache-2.0

//! Each deprecated way to open a reading against its replacement (#39): the same frames, the
//! same errors and the same log lines. Goes with the deprecated entry points, before 1.0.

#![allow(deprecated, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::io::Cursor;
use std::sync::{Arc, Mutex};

use common::{PFrame, Spec, TrackSpec, pack};
use scarletbook::audio::FRAME_BYTES_PER_CHANNEL;
use scarletbook::log::{Level, Log};
use scarletbook::{
    AreaIndex, AudioFrame, CursorOptions, Disc, Error, FrameSpan, IndexOptions, ResumedFrom, SeekPolicy,
    Slot, SpeculativeOptions, TrackAudioCursor, TrackCursor, TrackFrame,
};

const SLOT: Slot = Slot::TwoChannel;

type Image = Cursor<Vec<u8>>;

/// A frame range past the second track's start, within the play time.
const RANGE: (u32, u32) = (3, 6);

/// Plain DSD frames, two tracks over 6 frames: few sectors, as this file runs under Miri.
fn image() -> Vec<u8> {
    let fs: Vec<PFrame> = (0..6)
        .map(|k| PFrame { number: k, dst: false, data: vec![k as u8; 2 * FRAME_BYTES_PER_CHANNEL] })
        .collect();
    let tracks = vec![TrackSpec { start: 0, duration: 2 }, TrackSpec { start: 2, duration: 4 }];
    Spec { channels: 2, playtime: 6, tracks, audio: pack(&fs) }.build()
}

/// A disc of [`image`] that keeps every log line.
fn disc() -> (Disc<Image>, Arc<Mutex<Vec<String>>>) {
    let lines: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink = Arc::clone(&lines);
    let log = Log::new(Level::Trace, move |_, m| sink.lock().unwrap().push(m.to_string()));
    let disc = Disc::open_with_log(Cursor::new(image()), log).unwrap();
    lines.lock().unwrap().clear();
    (disc, lines)
}

/// A complete seek index of [`image`].
fn built_index() -> AreaIndex {
    let (mut disc, _) = disc();
    let area = disc.toc().areas[0].clone();
    let mut index = AreaIndex::new(&area, disc.toc().sectors, IndexOptions::new().with_interval(2)).unwrap();
    let options = CursorOptions::new().with_index(&index);
    let c = disc.frames_cursor(SLOT, FrameSpan::Range(0, 6), options).unwrap();
    let mut it = disc.read_frames(c);
    it.by_ref().for_each(|f| drop(f.unwrap()));
    let mut c = it.into_cursor();
    index.absorb(&mut c).unwrap();
    index
}

/// What a reading gives: its frames or the error opening it, and every line logged.
type Outcome<F> = (Result<Vec<F>, String>, Vec<String>);

fn audio(open: impl Fn(&Disc<Image>) -> Result<TrackAudioCursor, Error>) -> Outcome<AudioFrame> {
    let (mut disc, lines) = disc();
    let got = open(&disc)
        .map_err(|e| format!("{e:?}"))
        .map(|c| disc.read_track_audio(c).map(Result::unwrap).collect::<Vec<_>>());
    let lines = lines.lock().unwrap().clone();
    (got, lines)
}

fn frames(open: impl Fn(&Disc<Image>) -> Result<TrackCursor, Error>) -> Outcome<TrackFrame> {
    let (mut disc, lines) = disc();
    let got = open(&disc)
        .map_err(|e| format!("{e:?}"))
        .map(|c| disc.read_frames(c).map(Result::unwrap).collect::<Vec<_>>());
    let lines = lines.lock().unwrap().clone();
    (got, lines)
}

/// `Disc`'s `*_cursor` methods, for a track, a range, one that fails each way, and with the
/// caller's index.
#[test]
fn disc_cursors_match_their_replacements() {
    let none = CursorOptions::new;
    let (first, end) = RANGE;
    let range = FrameSpan::Range(first, end);
    for n in [2, 9] {
        assert_eq!(
            audio(|d| d.track_audio_cursor(SLOT, n)),
            audio(|d| d.audio_cursor(SLOT, FrameSpan::Track(n), none()))
        );
        assert_eq!(
            frames(|d| d.track_frames_cursor(SLOT, n)),
            frames(|d| d.frames_cursor(SLOT, FrameSpan::Track(n), none()))
        );
    }
    let (good, bad) =
        (audio(|d| d.range_audio_cursor(SLOT, first, end)), audio(|d| d.range_audio_cursor(SLOT, end, 9)));
    assert!(good.0.is_ok() && bad.0.is_err());
    assert_eq!(good, audio(|d| d.audio_cursor(SLOT, range, none())));
    assert_eq!(bad, audio(|d| d.audio_cursor(SLOT, FrameSpan::Range(end, 9), none())));
    assert_eq!(
        frames(|d| d.range_frames_cursor(SLOT, first, end)),
        frames(|d| d.frames_cursor(SLOT, range, none()))
    );

    let index = built_index();
    let with = none().with_index(&index);
    assert_eq!(
        audio(|d| d.range_audio_cursor_indexed(SLOT, first, end, &index)),
        audio(|d| d.audio_cursor(SLOT, range, with))
    );
    assert_eq!(
        frames(|d| d.range_frames_cursor_indexed(SLOT, first, end, &index)),
        frames(|d| d.frames_cursor(SLOT, range, with))
    );
    // An index of another image size is refused both ways, with the same line.
    let mut area = disc().0.toc().areas[0].clone();
    area.slot = SLOT;
    let other = AreaIndex::new(&area, disc().0.toc().sectors + 1, IndexOptions::new()).unwrap();
    let refused = frames(|d| d.range_frames_cursor_indexed(SLOT, first, end, &other));
    assert!(refused.0.is_err());
    assert_eq!(refused, frames(|d| d.frames_cursor(SLOT, range, none().with_index(&other))));
}

/// The constructors of cursors without a disc.
#[test]
fn free_cursors_match_their_replacements() {
    let (d, _) = disc();
    let (area, n) = (d.toc().areas[0].clone(), d.toc().sectors);
    let (first, end) = RANGE;
    let none = CursorOptions::new;
    assert_eq!(
        frames(|_| TrackCursor::for_range(&area, first, end, n)),
        frames(|_| TrackCursor::open_range(&area, first, end, n, none()))
    );
    assert_eq!(
        audio(|_| TrackAudioCursor::for_range(&area, first, end, n)),
        audio(|_| TrackAudioCursor::open_range(&area, first, end, n, none()))
    );
    let index = built_index();
    let with = none().with_index(&index);
    assert_eq!(
        frames(|_| TrackCursor::for_range_indexed(&area, first, end, n, &index)),
        frames(|_| TrackCursor::open_range(&area, first, end, n, with))
    );
    assert_eq!(
        audio(|_| TrackAudioCursor::for_range_indexed(&area, first, end, n, &index)),
        audio(|_| TrackAudioCursor::open_range(&area, first, end, n, with))
    );
    let o = SpeculativeOptions::new().with_min_gap_sectors(0);
    let policy = SeekPolicy::Speculative(o);
    assert_eq!(
        frames(|_| TrackCursor::for_range_speculative(&area, first, end, n, None, o)),
        frames(|_| TrackCursor::open_range(&area, first, end, n, none().with_policy(policy)))
    );
    assert_eq!(
        audio(|_| TrackAudioCursor::for_range_speculative(&area, first, end, n, Some(&index), o)),
        audio(|_| TrackAudioCursor::open_range(&area, first, end, n, with.with_policy(policy)))
    );
}

/// The borrowing iterators and `into_cursor` give the cursor of the shared-borrow form.
#[test]
fn iterators_and_cursors_are_the_same_reading() {
    let (mut a, lines_a) = disc();
    let (mut b, lines_b) = disc();
    let (first, end) = RANGE;
    let c = a.range_audio(SLOT, first, end).unwrap().into_cursor();
    let via_iterator: Vec<_> = a.read_track_audio(c).map(Result::unwrap).collect();
    let c = b.audio_cursor(SLOT, FrameSpan::Range(first, end), CursorOptions::new()).unwrap();
    let via_cursor: Vec<_> = b.read_track_audio(c).map(Result::unwrap).collect();
    assert_eq!(via_iterator, via_cursor);
    assert_eq!(*lines_a.lock().unwrap(), *lines_b.lock().unwrap());
}

/// A track can start from the caller's index, and a policy in the options reads as the disc's
/// would (on an image this small, whichever start it takes).
#[test]
fn options_replace_the_disc_defaults() {
    let policy = SeekPolicy::Speculative(SpeculativeOptions::new().with_min_gap_sectors(0));
    let range = FrameSpan::Range(RANGE.0, RANGE.1);
    let (mut a, lines_a) = disc();
    let c = a.frames_cursor(SLOT, range, CursorOptions::new().with_policy(policy)).unwrap();
    let via_options: Vec<_> = a.read_frames(c).map(Result::unwrap).collect();
    let (mut b, lines_b) = disc();
    b.set_seek_policy(SLOT, policy).unwrap();
    lines_b.lock().unwrap().clear();
    let c = b.frames_cursor(SLOT, range, CursorOptions::new()).unwrap();
    let via_disc: Vec<_> = b.read_frames(c).map(Result::unwrap).collect();
    assert_eq!(via_options, via_disc);
    assert_eq!(*lines_a.lock().unwrap(), *lines_b.lock().unwrap());

    let index = built_index();
    let track = FrameSpan::Track(2);
    let (from_index, lines) =
        frames(|d| d.frames_cursor(SLOT, track, CursorOptions::new().with_index(&index)));
    assert_eq!(from_index, frames(|d| d.frames_cursor(SLOT, track, CursorOptions::new())).0);
    assert!(lines.iter().any(|l| l.contains("track 2") && l.contains("from the seek index")), "{lines:?}");
}

/// A policy in the options takes the place of the disc's, both ways, on an image large enough
/// for a provisional start.
#[test]
#[cfg_attr(miri, ignore = "builds a large image; options_replace_the_disc_defaults covers the path")]
fn a_policy_in_the_options_decides_the_start() {
    let fs: Vec<PFrame> =
        (0..400).map(|k| PFrame { number: k, dst: true, data: vec![k as u8; 4000] }).collect();
    let tracks = vec![TrackSpec { start: 0, duration: 400 }];
    let img = Spec { channels: 2, playtime: 400, tracks, audio: pack(&fs) }.build();
    let policy = SeekPolicy::Speculative(SpeculativeOptions::new().with_min_gap_sectors(0));
    let range = FrameSpan::Range(380, 390);
    let read = |disc: &mut Disc<Image>, options| {
        let c = disc.frames_cursor(SLOT, range, options).unwrap();
        let mut it = disc.read_frames(c);
        let got: Vec<_> = it.by_ref().map(Result::unwrap).collect();
        (got, it.into_cursor().resumed_from())
    };

    let mut exact_disc = Disc::open(Cursor::new(img.clone())).unwrap();
    let (want, start) = read(&mut exact_disc, CursorOptions::new());
    assert!(matches!(start, ResumedFrom::Start), "{start:?}");
    let (got, start) = read(&mut exact_disc, CursorOptions::new().with_policy(policy));
    assert_eq!(got, want);
    assert!(matches!(start, ResumedFrom::Speculative { .. }), "{start:?}");

    let mut speculative_disc = Disc::open(Cursor::new(img)).unwrap();
    speculative_disc.set_seek_policy(SLOT, policy).unwrap();
    let (got, start) = read(&mut speculative_disc, CursorOptions::new());
    assert_eq!(got, want);
    assert!(matches!(start, ResumedFrom::Speculative { .. }), "{start:?}");
    let (got, start) = read(&mut speculative_disc, CursorOptions::new().with_policy(SeekPolicy::Exact));
    assert_eq!(got, want);
    assert!(matches!(start, ResumedFrom::Start), "{start:?}");
}
