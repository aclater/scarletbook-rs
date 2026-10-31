// SPDX-License-Identifier: MIT OR Apache-2.0

//! DSDIFF markers and the edited master (specification 8, 8.4.7, 8.4.8, 8.4.10 and
//! 8.4.11, at sacd-spec `6db1251`; issue #131): the `DIIN` chunk and its markers equal an
//! independent construction from those sections, the file around them is unchanged, and
//! the edited master's checks refuse what 8.4.11 excludes.

#![cfg(feature = "output")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;
mod output_common;

use std::io::{Cursor, Read, Seek};

use common::TrackSpec;
use output_common::{Lcg, check, contents, corpus, corpus_ranges, enough, plain, synthetic};
use scarletbook::output::{Format, LocalChunk, Marker, MarkerKind, Markers, OutputOptions, SourceTrack};
use scarletbook::toc::Area;
use scarletbook::{Disc, EditedMasterCheck, Error, OutputRefusal, Slot};

fn chunk(out: &mut Vec<u8>, id: &[u8], data: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&(data.len() as u64).to_be_bytes());
    out.extend_from_slice(data);
    if data.len() % 2 == 1 {
        out.push(0);
    }
}

/// The top-level local chunks of a DSDIFF file: (ID, whole chunk bytes).
fn locals(file: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
    assert_eq!(&file[..4], b"FRM8");
    assert_eq!(u64::from_be_bytes(file[4..12].try_into().unwrap()) + 12, file.len() as u64);
    assert_eq!(&file[12..16], b"DSD ");
    let mut at = 16;
    let mut out = Vec::new();
    while at < file.len() {
        let d = u64::from_be_bytes(file[at + 4..at + 12].try_into().unwrap()) as usize;
        let end = at + 12 + d + d % 2;
        out.push((file[at..at + 4].to_vec(), file[at..end].to_vec()));
        at = end;
    }
    assert_eq!(at, file.len());
    out
}

/// A marker's `MARK` chunk by 8.4.7, for frame `f` of a file of `[a, ..)` with absolute
/// start frame `start`.
fn mark(f: u32, a: u32, start: u32, kind: u16, text: &[u8], pad_counted: bool) -> Vec<u8> {
    let g = u64::from(start + f - a);
    let mut d = ((g / 270_000) as u16).to_be_bytes().to_vec();
    d.push((g / 4500 % 60) as u8);
    d.push((g / 75 % 60) as u8);
    d.extend_from_slice(&((g % 75 * 37_632) as u32).to_be_bytes());
    d.extend_from_slice(&[0; 4]);
    d.extend_from_slice(&kind.to_be_bytes());
    d.extend_from_slice(&[0, 0, 0, 0]);
    d.extend_from_slice(&(text.len() as u32).to_be_bytes());
    d.extend_from_slice(text);
    if pad_counted && text.len() % 2 == 1 {
        d.push(0);
    }
    let mut out = Vec::new();
    chunk(&mut out, b"MARK", &d);
    out
}

/// `DIIN` with `EMID` `emid` and markers (frame, kind) of a file of `[a, ..)`.
fn diin(emid: &[u8], markers: &[(u32, u16)], a: u32, start: u32) -> Vec<u8> {
    let mut d = Vec::new();
    chunk(&mut d, b"EMID", emid);
    for &(f, kind) in markers {
        d.extend_from_slice(&mark(f, a, start, kind, b"", false));
    }
    let mut out = Vec::new();
    chunk(&mut out, b"DIIN", &d);
    out
}

/// The program of the play range by 8.4.10's main text, or none (items 1 and 2).
fn play_program(area: &Area) -> Vec<(u32, u16)> {
    let t: Vec<(u32, u32)> =
        area.tracks.iter().map(|t| (t.start_frame, t.start_frame + t.duration_frames)).collect();
    let exact = t.iter().all(|&(s, e)| s < e) && t.windows(2).all(|w| w[0].1 <= w[1].0);
    if t.is_empty() || !exact {
        return Vec::new();
    }
    let mut m = vec![(0, 2)];
    for (i, &(s, e)) in t.iter().enumerate() {
        m.push((s, 0));
        if i + 1 == t.len() || e < t[i + 1].0 {
            m.push((e, 1));
        }
    }
    m
}

/// What 8.4.11 says of an edited master of an area.
fn edited_master_check(area: &Area) -> Option<EditedMasterCheck> {
    let t = &area.tracks;
    if area.channel_speakers().is_none() {
        Some(EditedMasterCheck::Layout)
    } else if t.is_empty() || t.len() > 255 {
        Some(EditedMasterCheck::TrackCount)
    } else if let Some(short) = t.iter().find(|t| t.duration_frames < 75) {
        Some(EditedMasterCheck::TrackTooShort { track: short.number })
    } else if !t.windows(2).all(|w| w[0].start_frame + w[0].duration_frames <= w[1].start_frame) {
        Some(EditedMasterCheck::TracksNotInOrder)
    } else if t.last().unwrap().end_frame() > (255 * 60 + 59) * 75 + 74 {
        Some(EditedMasterCheck::ProgramTooLong)
    } else {
        None
    }
}

/// The file of `layout` with `DIIN` taken out, and the `DIIN` chunk.
fn without_diin(file: &[u8]) -> (Vec<u8>, Option<Vec<u8>>) {
    let mut form = b"DSD ".to_vec();
    let mut found = None;
    for (id, bytes) in locals(file) {
        if id == b"DIIN" {
            found = Some(bytes);
        } else {
            form.extend_from_slice(&bytes);
        }
    }
    let mut out = Vec::new();
    chunk(&mut out, b"FRM8", &form);
    (out, found)
}

fn whole<R: Read + Seek>(
    disc: &mut Disc<R>,
    slot: Slot,
    first: u32,
    end: u32,
    options: &OutputOptions,
) -> Vec<u8> {
    let layout = disc.output_layout(slot, first, end, Format::DsdiffDsd, options).unwrap();
    let mut f = vec![0; layout.size() as usize];
    layout.read_at(disc, 0, &mut f).unwrap();
    f
}

#[test]
#[cfg_attr(miri, ignore = "the corpus is too large for Miri")]
fn corpus_markers_and_edited_masters() {
    let mut rng = Lcg(810);
    let mut checked = 0;
    for (_name, bytes) in corpus() {
        let Ok(mut disc) = Disc::open(Cursor::new(bytes)) else { continue };
        for area in disc.toc().areas.clone() {
            if area.channel_speakers().is_none()
                || disc.check_range(area.slot, 0, area.playtime_frames()).is_err()
            {
                continue;
            }
            let p = area.playtime_frames();
            // The play range with `DIIN`: the program of 8.4.10, the rest of the file as without.
            let plain_file = whole(&mut disc, area.slot, 0, p, &OutputOptions::default());
            let options = OutputOptions::default()
                .with_dsdiff_edited_master_info(true)
                .with_dsdiff_edited_master_id("id-1".into());
            let layout = disc.output_layout(area.slot, 0, p, Format::DsdiffDsd, &options).unwrap();
            let expected_diin = diin(b"id-1", &play_program(&area), 0, 0);
            let (rest, found) = without_diin(&whole(&mut disc, area.slot, 0, p, &options));
            assert_eq!(rest, plain_file);
            assert_eq!(found.as_deref(), Some(&expected_diin[..]));
            let ids: Vec<Vec<u8>> =
                locals(&whole(&mut disc, area.slot, 0, p, &options)).into_iter().map(|(id, _)| id).collect();
            assert_eq!(ids, [b"FVER".to_vec(), b"PROP".to_vec(), b"DIIN".to_vec(), b"DSD ".to_vec()]);
            let content = contents(&mut disc, area.slot, 0, p);
            let expected = whole(&mut disc, area.slot, 0, p, &options);
            check(&mut disc, &layout, &expected, &content, &mut rng);
            checked += 1;

            // The edited master: refused as 8.4.11 says, or the program with `ABSS` and `LSCO`.
            let master = OutputOptions::default().with_dsdiff_edited_master(true);
            match (
                edited_master_check(&area),
                disc.output_layout(area.slot, 0, p, Format::DsdiffDsd, &master),
            ) {
                (Some(want), Err(Error::OutputRefused(OutputRefusal::NotEditedMaster(got)))) => {
                    assert_eq!(got, want)
                }
                (None, Ok(_)) => {
                    let f = whole(&mut disc, area.slot, 0, p, &master);
                    let prop = &locals(&f)[1].1;
                    assert!(prop.windows(4).any(|w| w == b"ABSS") && prop.windows(4).any(|w| w == b"LSCO"));
                    assert_eq!(without_diin(&f).1.unwrap(), diin(b"", &play_program(&area), 0, 0));
                }
                (want, got) => panic!("{want:?} but {:?}", got.map(|l| l.size())),
            }
            if p > 0 && area.tracks.len() > 1 {
                let e = disc.output_layout(area.slot, 0, p - 1, Format::DsdiffDsd, &master).unwrap_err();
                assert!(
                    matches!(
                        e,
                        Error::OutputRefused(OutputRefusal::NotEditedMaster(EditedMasterCheck::Range))
                    ),
                    "{e}"
                );
            }

            // Every other range: one-track programs of source tracks (W28), clipped
            // programs of frame ranges (W30), none by default.
            for (first, end) in corpus_ranges(&disc, area.slot) {
                if disc.check_range(area.slot, first, end).is_err() || (first, end) == (0, p) {
                    continue;
                }
                let on = OutputOptions::default().with_dsdiff_edited_master_info(true);
                let none = without_diin(&whole(&mut disc, area.slot, first, end, &on)).1.unwrap();
                assert_eq!(none, diin(b"", &[], first, 0));
                let layout = disc.output_layout(area.slot, first, end, Format::DsdiffDsd, &on).unwrap();
                let want: Vec<(u32, u16)> = match layout.source_track().and_then(|n| area.track(u32::from(n)))
                {
                    Some(t) => {
                        let (s, e) = (first.max(t.start_frame), end.min(t.end_frame()));
                        if s < e { vec![(first, 2), (s, 0), (e, 1)] } else { vec![] }
                    }
                    None => clipped(&area, first, end),
                };
                let o = on.clone().with_dsdiff_source_track_markers(true).with_dsdiff_clipped_program(true);
                let got = without_diin(&whole(&mut disc, area.slot, first, end, &o)).1.unwrap();
                assert_eq!(got, diin(b"", &want, first, 0), "range {first}..{end}");
            }
        }
    }
    assert!(checked > enough() / 5, "only {checked} areas checked");
}

/// The clipped program of 8.4.10 item 4.
fn clipped(area: &Area, a: u32, b: u32) -> Vec<(u32, u16)> {
    let t: Vec<(u32, u32)> = area
        .tracks
        .iter()
        .map(|t| (a.max(t.start_frame), b.min(t.start_frame + t.duration_frames)))
        .filter(|&(s, e)| s < e)
        .collect();
    let exact = t.windows(2).all(|w| w[0].1 <= w[1].0);
    if t.is_empty() || !exact {
        return Vec::new();
    }
    let mut m = vec![(a, 2)];
    for (i, &(s, e)) in t.iter().enumerate() {
        m.push((s, 0));
        if i + 1 == t.len() || e < t[i + 1].0 {
            m.push((e, 1));
        }
    }
    m
}

#[test]
fn custom_markers_absolute_start_and_padding() {
    let mut disc = synthetic(
        2,
        &plain(2, 4),
        vec![TrackSpec { start: 0, duration: 2 }, TrackSpec { start: 2, duration: 2 }],
        4,
    );
    let mut m = Marker::new(1, MarkerKind::Index);
    m.text = b"odd".to_vec();
    let mut start = Marker::new(2, MarkerKind::TrackStart);
    start.track_flags = 7;
    start.text = b"T2".to_vec();
    let mut stop = Marker::new(4, MarkerKind::TrackStop);
    stop.track_flags = 7;
    for counted in [false, true] {
        let options = OutputOptions::default()
            .with_dsdiff_edited_master_info(true)
            .with_dsdiff_absolute_start(Some(270_000 - 1))
            .with_dsdiff_text_pad_counted(counted)
            .with_dsdiff_markers(Markers::Custom(vec![m.clone(), start.clone(), stop.clone()]));
        let f = whole(&mut disc, Slot::TwoChannel, 1, 4, &options);
        let mut d = Vec::new();
        chunk(&mut d, b"EMID", b"");
        d.extend_from_slice(&mark(1, 1, 270_000 - 1, 4, b"odd", counted));
        let mut with_flags = mark(2, 1, 270_000 - 1, 0, b"T2", counted);
        with_flags[12 + 16..12 + 18].copy_from_slice(&7u16.to_be_bytes());
        d.extend_from_slice(&with_flags);
        // Track flags only on a track start (8.4.7 item 4).
        d.extend_from_slice(&mark(4, 1, 270_000 - 1, 1, b"", counted));
        let mut want = Vec::new();
        chunk(&mut want, b"DIIN", &d);
        assert_eq!(without_diin(&f).1.unwrap(), want);
        // The marker at frame 2 is 1 h 0 min 0 s, 0 samples (8.4.7 item 1).
        let at = want.windows(4).position(|w| w == b"MARK").unwrap();
        let second = want[at + 12..].windows(4).position(|w| w == b"MARK").unwrap() + at + 12;
        assert_eq!(&want[second + 12..second + 20], &[0, 1, 0, 0, 0, 0, 0, 0]);
    }
    // Markers outside the range, text that isn't text bytes, and an `EMID` that isn't.
    let bad = [
        OutputOptions::default()
            .with_dsdiff_edited_master_info(true)
            .with_dsdiff_markers(Markers::Custom(vec![Marker::new(0, MarkerKind::TrackStart)])),
        OutputOptions::default().with_dsdiff_edited_master_info(true).with_dsdiff_markers(Markers::Custom(
            vec![{
                let mut m = Marker::new(2, MarkerKind::Index);
                m.text = vec![0x7F];
                m
            }],
        )),
        OutputOptions::default()
            .with_dsdiff_edited_master_info(true)
            .with_dsdiff_edited_master_id("é".into()),
    ];
    for options in bad {
        let e = disc.output_layout(Slot::TwoChannel, 1, 4, Format::DsdiffDsd, &options).unwrap_err();
        assert!(matches!(e, Error::InvalidArgument(_)), "{e}");
    }
    // `DIIN` can go anywhere after `FVER` (W13), and is ignored by DSF.
    let order = vec![LocalChunk::SoundData, LocalChunk::Property, LocalChunk::EditedMasterInfo];
    let o = OutputOptions::default().with_dsdiff_edited_master_info(true).with_dsdiff_local_order(order);
    assert!(
        disc.output_layout(Slot::TwoChannel, 1, 4, Format::DsdiffDsd, &o).is_err(),
        "PROP after the sound data"
    );
    let order = vec![LocalChunk::Property, LocalChunk::SoundData, LocalChunk::EditedMasterInfo];
    let o = OutputOptions::default().with_dsdiff_edited_master_info(true).with_dsdiff_local_order(order);
    let ids: Vec<Vec<u8>> =
        locals(&whole(&mut disc, Slot::TwoChannel, 1, 4, &o)).into_iter().map(|(id, _)| id).collect();
    assert_eq!(ids, [b"FVER".to_vec(), b"PROP".to_vec(), b"DSD ".to_vec(), b"DIIN".to_vec()]);
    let dsf =
        disc.output_layout(Slot::TwoChannel, 1, 4, Format::Dsf, &o.with_dsdiff_edited_master(true)).unwrap();
    assert_eq!(dsf.size(), 92 + 4096 * 4 * 2);
}

#[test]
fn source_tracks() {
    let disc = synthetic(
        2,
        &plain(2, 1),
        vec![TrackSpec { start: 0, duration: 2 }, TrackSpec { start: 2, duration: 2 }],
        6,
    );
    let layout = |first, end, choice| {
        disc.output_layout(
            Slot::TwoChannel,
            first,
            end,
            Format::Dsf,
            &OutputOptions::default().with_source_track(choice),
        )
    };
    assert_eq!(layout(0, 2, SourceTrack::Auto).unwrap().source_track(), Some(1));
    // Track 2 with pause runs to the play time.
    assert_eq!(layout(2, 6, SourceTrack::Auto).unwrap().source_track(), Some(2));
    assert_eq!(layout(1, 3, SourceTrack::Auto).unwrap().source_track(), None);
    assert_eq!(layout(0, 6, SourceTrack::Auto).unwrap().source_track(), None);
    assert_eq!(layout(0, 2, SourceTrack::None).unwrap().source_track(), None);
    assert_eq!(layout(1, 3, SourceTrack::Track(2)).unwrap().source_track(), Some(2));
    assert!(matches!(layout(0, 2, SourceTrack::Track(2)).unwrap_err(), Error::InvalidArgument(_)));
    assert!(matches!(layout(0, 2, SourceTrack::Track(3)).unwrap_err(), Error::NoSuchTrack { .. }));
}

#[test]
#[cfg_attr(miri, ignore = "75-frame tracks are too large for Miri")]
fn edited_master_refusals() {
    let tracks = |v: &[(u32, u32)]| {
        v.iter().map(|&(start, duration)| TrackSpec { start, duration }).collect::<Vec<_>>()
    };
    let master = OutputOptions::default().with_dsdiff_edited_master(true);
    type Case<'a> = (&'a [(u32, u32)], u32, Option<EditedMasterCheck>);
    let cases: &[Case<'_>] = &[
        (&[(0, 75), (75, 75)], 160, None),
        (&[(5, 75), (85, 75)], 160, None),
        (&[(0, 75), (70, 80)], 160, Some(EditedMasterCheck::TracksNotInOrder)),
        (&[(0, 75), (75, 74)], 160, Some(EditedMasterCheck::TrackTooShort { track: 2 })),
        (&[(80, 75), (0, 75)], 160, Some(EditedMasterCheck::TracksNotInOrder)),
    ];
    for (t, p, want) in cases {
        let mut disc = synthetic(2, &plain(2, *p), tracks(t), *p);
        let got = disc.output_layout(Slot::TwoChannel, 0, *p, Format::DsdiffDsd, &master);
        match (want, got) {
            (None, Ok(_)) => {
                let area = disc.toc().area(Slot::TwoChannel).unwrap().clone();
                let f = whole(&mut disc, Slot::TwoChannel, 0, *p, &master);
                assert_eq!(without_diin(&f).1.unwrap(), diin(b"", &play_program(&area), 0, 0));
            }
            (Some(w), Err(Error::OutputRefused(OutputRefusal::NotEditedMaster(g)))) => assert_eq!(&g, w),
            (w, g) => panic!("{t:?}: {w:?} but {:?}", g.map(|l| l.size())),
        }
    }
    // An unspecified layout, and custom markers.
    let disc = synthetic(4, &plain(4, 75), tracks(&[(0, 75)]), 75);
    let e = disc.output_layout(Slot::TwoChannel, 0, 75, Format::DsdiffDsd, &master).unwrap_err();
    assert!(
        matches!(e, Error::OutputRefused(OutputRefusal::NotEditedMaster(EditedMasterCheck::Layout))),
        "{e}"
    );
    let disc = synthetic(2, &plain(2, 75), tracks(&[(0, 75)]), 75);
    let no_lsco = master.clone().with_dsdiff_loudspeaker_configuration(false);
    let e = disc.output_layout(Slot::TwoChannel, 0, 75, Format::DsdiffDsd, &no_lsco).unwrap_err();
    assert!(matches!(e, Error::InvalidArgument(_)), "{e}");
    let custom = master.clone().with_dsdiff_markers(Markers::Custom(Vec::new()));
    assert!(matches!(
        disc.output_layout(Slot::TwoChannel, 0, 75, Format::DsdiffDsd, &custom).unwrap_err(),
        Error::InvalidArgument(_)
    ));
}
