// SPDX-License-Identifier: MIT OR Apache-2.0

//! Undecoded frames over the corpus: each DST frame's bytes, decoded on their own, give
//! the decoded read's audio, and both reads see the same positions and sectors.

use std::fs::File;
use std::io::BufReader;

use scarletbook::audio::{FRAME_BYTES_PER_CHANNEL, INVALID_FILL, MAX_FRAME_BYTES, MISSING_FILL};
use scarletbook::{AudioFrame, Disc, DstDecoder, FrameContent, FrameStatus, Slot, TrackCursor, TrackFrame};
use scarletbook_conformance::{corpus_dir, load_cases};

#[test]
fn raw_frames_decode_to_the_decoded_read() {
    let dir = corpus_dir();
    let cases = load_cases(&dir).unwrap_or_else(|e| panic!("manifest: {e}"));
    let mut decoder = DstDecoder::new();
    let (mut areas, mut dst, mut invalid, mut malformed) = (0, 0, 0, 0);
    for case in &cases {
        let id = case["id"].as_str().unwrap_or("?");
        let file = File::open(dir.join("images").join(format!("{id}.iso"))).unwrap();
        let Ok(mut disc) = Disc::open(BufReader::new(file)) else { continue };
        for slot in Slot::ALL {
            let Some(area) = disc.toc().area(slot) else { continue };
            let (playtime, channels) = (area.playtime_frames(), area.channel_count);
            if disc.check_range(slot, 0, playtime).is_err() {
                assert!(disc.range_frames(slot, 0, playtime).is_err());
                continue;
            }
            let mut raw_iter = disc.range_frames(slot, 0, playtime).unwrap();
            let raw: Vec<TrackFrame> = raw_iter.by_ref().map(Result::unwrap).collect();
            let raw_missing = raw_iter.missing_frames();
            let raw_cursor = raw_iter.into_cursor();
            let mut audio = disc.range_audio(slot, 0, playtime).unwrap();
            let decoded: Vec<AudioFrame> = audio.by_ref().map(Result::unwrap).collect();
            assert_eq!(raw_missing, audio.missing_frames(), "{id} {slot}");
            assert_eq!(raw_cursor.malformed_sectors(), audio.malformed_sectors(), "{id} {slot}");
            assert_eq!(raw_cursor.warnings(), audio.warnings(), "{id} {slot}");
            malformed += usize::from(raw_cursor.malformed_sectors() > 0);
            assert_eq!(raw.len(), decoded.len(), "{id} {slot}");
            let size = usize::from(channels) * FRAME_BYTES_PER_CHANNEL;
            for (r, d) in raw.iter().zip(&decoded) {
                assert_eq!(r.number, d.number, "{id} {slot}");
                let at = format!("{id} {slot} frame {}", r.number);
                // The DST equivalent of every position decodes to its audio (4.6.1 item 2).
                let mut out = vec![0u8; size];
                let equivalent = r.content.dst_equivalent(channels).unwrap();
                assert!(equivalent.len() <= MAX_FRAME_BYTES, "{at}");
                match decoder.decode(&equivalent, channels, &mut out) {
                    Ok(()) => assert_eq!(out, d.data, "{at}"),
                    Err(e) => assert_eq!(d.status, FrameStatus::Invalid(e), "{at}"),
                }
                match &r.content {
                    FrameContent::Dsd(bytes) => {
                        assert_eq!(d.status, FrameStatus::Dsd, "{at}");
                        assert_eq!(bytes, &d.data, "{at}");
                    }
                    FrameContent::Missing => {
                        assert_eq!(d.status, FrameStatus::Missing, "{at}");
                        assert!(d.data.iter().all(|&b| b == MISSING_FILL), "{at}");
                    }
                    FrameContent::Dst(bytes) => {
                        assert!((1..=MAX_FRAME_BYTES).contains(&bytes.len()), "{at}");
                        let mut out = vec![0u8; size];
                        match decoder.decode(bytes, channels, &mut out) {
                            Ok(()) => {
                                assert_eq!(d.status, FrameStatus::Decoded, "{at}");
                                assert_eq!(out, d.data, "{at}");
                            }
                            Err(e) => {
                                assert_eq!(d.status, FrameStatus::Invalid(e), "{at}");
                                assert!(d.data.iter().all(|&b| b == INVALID_FILL), "{at}");
                                invalid += 1;
                            }
                        }
                        dst += 1;
                    }
                }
            }
            check_tracks(&mut disc, slot, &raw, id);
            check_list(&mut disc, slot, &raw, id);
            areas += 1;
        }
    }
    eprintln!(
        "areas: {areas}, DST frames: {dst} ({invalid} invalid), areas with malformed sectors: {malformed}"
    );
    assert!(areas > 100 && dst > 100 && invalid > 0 && malformed > 0);
}

/// The stored frame list (4.6.1 item 2) holds the frames of the play range's stored content
/// in order, and any frames numbered at or past the play time after them.
fn check_list(disc: &mut Disc<BufReader<File>>, slot: Slot, range: &[TrackFrame], id: &str) {
    let playtime = range.len() as u32;
    let mut list = disc.stored_frame_list(slot).unwrap();
    let frames: Vec<scarletbook::AreaFrame> = list.by_ref().map(Result::unwrap).collect();
    let in_range: Vec<TrackFrame> =
        range.iter().filter(|f| f.content != FrameContent::Missing).cloned().collect();
    let listed: Vec<TrackFrame> = frames
        .iter()
        .filter(|f| f.number < playtime)
        .map(|f| TrackFrame {
            number: f.number,
            content: match f.kind {
                scarletbook::FrameKind::Dst => FrameContent::Dst(f.data.clone()),
                scarletbook::FrameKind::Dsd => FrameContent::Dsd(f.data.clone()),
            },
        })
        .collect();
    assert_eq!(listed, in_range, "{id} {slot}");
    assert!(frames.windows(2).all(|w| w[0].number < w[1].number), "{id} {slot}");
}

/// Each track's raw frames, read with one cursor continued through the area's tracks in
/// order, are that slice of the whole area's.
fn check_tracks(disc: &mut Disc<BufReader<File>>, slot: Slot, whole: &[TrackFrame], id: &str) {
    let Some(area) = disc.toc().area(slot) else { return };
    let tracks: Vec<(u32, u32, u32)> =
        area.tracks.iter().map(|t| (u32::from(t.number), t.start_frame, t.end_frame())).collect();
    let mut cursor: Option<TrackCursor> = None;
    for (n, start, end) in tracks {
        if disc.check_track(slot, n).is_err() {
            continue;
        }
        let next = match cursor.take() {
            Some(prev) => disc.continue_track_frames(prev, slot, n).unwrap(),
            None => disc.track_frames_cursor(slot, n).unwrap(),
        };
        let mut frames = disc.read_frames(next);
        let got: Vec<TrackFrame> = frames.by_ref().map(Result::unwrap).collect();
        assert_eq!(&got[..], &whole[start as usize..end as usize], "{id} {slot} track {n}");
        cursor = Some(frames.into_cursor());
    }
}
