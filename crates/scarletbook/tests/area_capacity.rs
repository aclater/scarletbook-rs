// SPDX-License-Identifier: MIT OR Apache-2.0

//! `Disc::area_capacity` and `audio::area_capacity` (#340): the figures of synthetic and
//! corpus areas, and the plain bound against the plain frames each area actually carries.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::io::Cursor;
use std::path::PathBuf;

use common::{PFrame, Spec, TrackSpec, pack};
use scarletbook::audio::{self, FRAME_BYTES_PER_CHANNEL};
use scarletbook::{AreaCapacity, Disc, Error, FrameKind, Slot};

/// The figures of the area from `slot`, checked against `audio::area_capacity`, and the
/// number of plain frames the area carries.
fn figures<R: std::io::Read + std::io::Seek>(disc: &mut Disc<R>, slot: Slot) -> Option<(AreaCapacity, u64)> {
    let c = match disc.area_capacity(slot) {
        Ok(c) => c,
        Err(Error::NoSuchArea(_)) => return None,
        Err(e) => panic!("{slot}: {e}"),
    };
    let n = disc.image_mut().sector_count();
    assert_eq!(audio::area_capacity(disc.toc().area(slot).unwrap(), n), c);
    assert_eq!(c.capacity, 7 * c.audio_sectors);
    assert!(!c.frames_per_sector().is_nan());
    let plain =
        disc.area_frames(slot).unwrap().map(|f| f.unwrap()).filter(|f| f.kind == FrameKind::Dsd).count();
    Some((c, plain as u64))
}

/// Synthetic plain areas, small enough for Miri: the figures, and the plain frames carried
/// against the bound; then the same sectors under a TOC that claims far more frames.
#[test]
fn synthetic_areas() {
    for (channels, frames) in [(2u8, 3u32), (6, 2)] {
        let fs: Vec<PFrame> = (0..frames)
            .map(|k| PFrame {
                number: k,
                dst: false,
                data: vec![k as u8; usize::from(channels) * FRAME_BYTES_PER_CHANNEL],
            })
            .collect();
        let audio = pack(&fs);
        let sectors = audio.len() as u64;
        let tracks = vec![TrackSpec { start: 0, duration: 1 }, TrackSpec { start: 1, duration: frames - 1 }];
        let spec = Spec { channels, playtime: frames, tracks, audio };
        let mut disc = Disc::open(Cursor::new(spec.build())).unwrap();
        let (c, plain) = figures(&mut disc, Slot::TwoChannel).unwrap();
        assert_eq!((c.audio_sectors, c.playtime_frames, c.max_track_end), (sectors, frames, frames));
        let bound = sectors * 2045 / (u64::from(channels) * 4704);
        assert_eq!(c.plain_frame_bound, Some(bound));
        assert!(u64::from(frames) == plain && plain <= bound, "{channels} channels: {plain} > {bound}");
        assert_eq!(c.frames_per_sector(), f64::from(frames) / sectors as f64);
        assert!(matches!(disc.area_capacity(Slot::Multichannel), Err(Error::NoSuchArea(Slot::Multichannel))));

        // The same sectors, with a TOC claiming an hour: still within the capacity of 7
        // frames per sector for a short track, far outside the plain bound.
        let hour = 75 * 3600;
        let tracks = vec![TrackSpec { start: 0, duration: 2 }, TrackSpec { start: 2, duration: hour - 2 }];
        let spec = Spec { playtime: hour, tracks, ..spec };
        let mut disc = Disc::open(Cursor::new(spec.build())).unwrap();
        let (inflated, plain) = figures(&mut disc, Slot::TwoChannel).unwrap();
        assert_eq!(
            (inflated.audio_sectors, inflated.plain_frame_bound),
            (c.audio_sectors, c.plain_frame_bound)
        );
        assert_eq!(
            (inflated.playtime_frames, inflated.max_track_end, plain),
            (hour, hour, u64::from(frames))
        );
        assert_eq!(inflated.frames_per_sector(), f64::from(hour) / sectors as f64);
        assert!(inflated.frames_per_sector() > 7.0);
    }
}

/// Every corpus image that opens, hostile or not: no area carries more plain frames than
/// its plain bound, and every valid image's areas claim at most one frame per sector.
#[test]
#[cfg_attr(miri, ignore = "reads every corpus image; synthetic_areas and the unit tests cover the figures")]
fn corpus_areas() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus");
    // The manifest's cases, each `"id": "...",` then `"category": "...",`, without a JSON
    // parser (this crate has none).
    let manifest = std::fs::read_to_string(root.join("manifest.json")).unwrap();
    let valid: Vec<&str> = manifest
        .split("\"id\": \"")
        .skip(1)
        .filter(|case| {
            let t: Vec<_> = case.split_whitespace().take(3).collect();
            t[1..] == ["\"category\":", "\"valid\","]
        })
        .map(|case| &case[..case.find('"').unwrap()])
        .collect();
    assert_eq!(valid.len(), 33);
    let mut paths: Vec<_> =
        std::fs::read_dir(root.join("images")).unwrap().map(|e| e.unwrap().path()).collect();
    paths.sort();
    let (mut areas, mut bounded, mut valid_areas, mut largest) = (0, 0, 0, 0f64);
    for p in paths {
        let id = p.file_stem().unwrap().to_string_lossy().into_owned();
        let Ok(mut disc) = Disc::open(Cursor::new(std::fs::read(&p).unwrap())) else { continue };
        for slot in [Slot::TwoChannel, Slot::Multichannel] {
            let Some((c, plain)) = figures(&mut disc, slot) else { continue };
            areas += 1;
            if let Some(bound) = c.plain_frame_bound {
                assert!(plain <= bound, "{id} {slot}: {plain} plain frames, bound {bound}");
                bounded += 1;
            }
            if valid.contains(&id.as_str()) {
                valid_areas += 1;
                largest = largest.max(c.frames_per_sector());
            }
        }
    }
    assert!(areas > 150 && bounded > 150 && valid_areas > 30, "{areas} {bounded} {valid_areas}");
    // dst-features-1ch's multichannel area: 37 frames in 41 sectors.
    assert_eq!(largest, 37.0 / 41.0);
}
