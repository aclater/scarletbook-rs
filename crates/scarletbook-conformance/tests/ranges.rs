// SPDX-License-Identifier: MIT OR Apache-2.0

//! Frame ranges over the corpus: an area's whole timeline is its tracks plus the frames
//! between and around them.

use std::fs::File;
use std::io::BufReader;

use scarletbook::{AudioFrame, Disc, Error, Slot, TrackAudioCursor};
use scarletbook_conformance::{corpus_dir, load_cases};

type Image = Disc<BufReader<File>>;

fn read(disc: &mut Image, cursor: TrackAudioCursor) -> (Vec<AudioFrame>, TrackAudioCursor) {
    let mut audio = disc.read_track_audio(cursor);
    let frames = audio.by_ref().map(|f| f.unwrap_or_else(|e| panic!("{e}"))).collect();
    (frames, audio.into_cursor())
}

/// The pieces that tile `[0, P)`: tracks with audio in start order (skipping any that
/// overlap an earlier one), and the ranges between them.
#[derive(Clone, Copy, Debug)]
enum Piece {
    Track(u32),
    Range(u32, u32),
}

fn pieces(disc: &Image, slot: Slot) -> Vec<Piece> {
    let Some(area) = disc.toc().area(slot) else { return Vec::new() };
    let mut tracks: Vec<_> = area
        .tracks
        .iter()
        .filter(|t| disc.check_track(slot, u32::from(t.number)).is_ok())
        .map(|t| (t.start_frame, t.end_frame(), u32::from(t.number)))
        .collect();
    tracks.sort_unstable();
    let mut out = Vec::new();
    let mut at = 0;
    for (start, end, n) in tracks {
        if start < at {
            continue;
        }
        if start > at {
            out.push(Piece::Range(at, start));
        }
        out.push(Piece::Track(n));
        at = end;
    }
    if at < area.playtime_frames() {
        out.push(Piece::Range(at, area.playtime_frames()));
    }
    out
}

#[test]
fn whole_area_is_tracks_plus_gaps() {
    let dir = corpus_dir();
    let cases = load_cases(&dir).unwrap_or_else(|e| panic!("manifest: {e}"));
    let (mut areas, mut with_gaps, mut with_pause) = (0, 0, 0);
    for case in &cases {
        let id = case["id"].as_str().unwrap_or("?");
        let file = File::open(dir.join("images").join(format!("{id}.iso"))).unwrap();
        let Ok(mut disc) = Disc::open(BufReader::new(file)) else { continue };
        for slot in Slot::ALL {
            let Some(area) = disc.toc().area(slot) else { continue };
            let playtime = area.playtime_frames();
            let channels = area.channel_count;
            let whole = match disc.range_audio_cursor(slot, 0, playtime) {
                Ok(c) => c,
                Err(Error::TrackEmpty) => {
                    assert_eq!(playtime, 0, "{id} {slot}");
                    continue;
                }
                Err(Error::ChannelCountUnsupported { .. }) => {
                    assert!(!(1..=6).contains(&channels), "{id} {slot}");
                    continue;
                }
                // The capacity check, as for tracks: no track of such an area may end
                // past the capacity either.
                Err(Error::TrackExceedsCapacity { capacity, .. }) => {
                    assert!(u64::from(playtime) > capacity, "{id} {slot}");
                    continue;
                }
                Err(e) => panic!("{id} {slot}: {e}"),
            };
            let (all, _) = read(&mut disc, whole);
            assert_eq!(all.len() as u32, playtime, "{id} {slot}");
            assert!(all.iter().zip(0..).all(|(f, n)| f.number == n), "{id} {slot}");

            // Tracks with pauses (4.7.1 item 5) are slices of the play range too.
            let count = disc.toc().area(slot).map_or(0, |a| a.tracks.len()) as u32;
            for n in 1..=count {
                let (a, b) = disc.track_range_with_pause(slot, n).unwrap();
                match disc.range_audio(slot, a, b) {
                    Ok(audio) => {
                        let got: Vec<AudioFrame> =
                            audio.map(|f| f.unwrap_or_else(|e| panic!("{e}"))).collect();
                        assert_eq!(got, all[a as usize..b as usize], "{id} {slot} track {n} [{a}, {b})");
                        with_pause += 1;
                    }
                    Err(Error::TrackEmpty) => assert_eq!(a, b, "{id} {slot} track {n}"),
                    Err(e) => panic!("{id} {slot} track {n} [{a}, {b}): {e}"),
                }
            }

            // Fresh cursors per piece, and one cursor continued through all of them.
            let pieces = pieces(&disc, slot);
            with_gaps += usize::from(pieces.iter().any(|p| matches!(p, Piece::Range(..))));
            let mut fresh = Vec::new();
            let mut continued = Vec::new();
            let mut cursor: Option<TrackAudioCursor> = None;
            for &p in &pieces {
                let (c, next) = match p {
                    Piece::Track(n) => (
                        disc.track_audio_cursor(slot, n).unwrap(),
                        match cursor.take() {
                            Some(prev) => disc.continue_track_audio(prev, slot, n).unwrap(),
                            None => disc.track_audio_cursor(slot, n).unwrap(),
                        },
                    ),
                    Piece::Range(a, b) => (
                        disc.range_audio_cursor(slot, a, b).unwrap(),
                        match cursor.take() {
                            Some(prev) => disc.continue_range_audio(prev, slot, a, b).unwrap(),
                            None => disc.range_audio_cursor(slot, a, b).unwrap(),
                        },
                    ),
                };
                fresh.extend(read(&mut disc, c).0);
                let (frames, back) = read(&mut disc, next);
                continued.extend(frames);
                cursor = Some(back);
            }
            assert_eq!(fresh, all, "{id} {slot} {pieces:?}");
            assert_eq!(continued, all, "{id} {slot} {pieces:?}");
            areas += 1;
        }
    }
    eprintln!(
        "areas compared: {areas}, with frames outside tracks: {with_gaps}, tracks with pauses: {with_pause}"
    );
    assert!(areas > 100 && with_gaps > 0 && with_pause > 300, "{areas} {with_gaps} {with_pause}");
}
