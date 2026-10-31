// SPDX-License-Identifier: MIT OR Apache-2.0

//! DSDIFF markers and the edited master information chunk (specification 8, 8.4.7, 8.4.8,
//! 8.4.10 and 8.4.11).

use super::dsdiff::{MAX_TIME_FRAMES, chunk, time};
use super::{Layout, OutputOptions};
use crate::error::{EditedMasterCheck, Error, OutputRefusal};
use crate::toc::{Area, LoudspeakerLayout, Track};

/// The kind of a DSDIFF marker (specification 8, 8.4.7 item 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MarkerKind {
    /// Track start (0).
    TrackStart,
    /// Track stop (1).
    TrackStop,
    /// Program start (2).
    ProgramStart,
    /// Index (4).
    Index,
}

impl MarkerKind {
    fn value(self) -> u16 {
        match self {
            MarkerKind::TrackStart => 0,
            MarkerKind::TrackStop => 1,
            MarkerKind::ProgramStart => 2,
            MarkerKind::Index => 4,
        }
    }
}

/// A DSDIFF marker (specification 8, 8.4.7), at a frame of the area's timeline. Its time
/// is that of frame `A + frame - first` (8.4.8), its offset and channel 0.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Marker {
    /// The frame of the area's timeline, from the range's first to its end, inclusive.
    pub frame: u32,
    /// Its kind.
    pub kind: MarkerKind,
    /// Track flags (8.4.7 item 4): meaningful only on a track start, 0 otherwise. The TOC
    /// defines none, so markers made from it have 0 (W23).
    pub track_flags: u16,
    /// Marker text: text bytes 0x20 to 0x7E only (8.4.1 item 4).
    pub text: Vec<u8>,
}

impl Marker {
    /// A marker of `kind` at `frame`, with no track flags and no text.
    pub fn new(frame: u32, kind: MarkerKind) -> Self {
        Marker { frame, kind, track_flags: 0, text: Vec::new() }
    }
}

/// Which markers a DSDIFF file holds when it has an edited master information chunk
/// (specification 8, 8.4.10).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Markers {
    /// The markers 8.4.10 gives the file: the program of the play range; for a file with a
    /// source track, a one-track program when [`OutputOptions::dsdiff_source_track_markers`]
    /// is set (W28); for a frame range without one, the clipped program when
    /// [`OutputOptions::dsdiff_clipped_program`] is set (W30); otherwise none. Tracks that
    /// are out of order, overlap or are empty get none (8.4.10 item 2; W27).
    FromToc,
    /// These markers, in this order.
    Custom(Vec<Marker>),
}

/// The tracks of an area with each one's `[s_i, s_i + d_i)`.
fn spans(area: &Area) -> impl Iterator<Item = (&Track, u32, u32)> {
    area.tracks.iter().map(|t| (t, t.start_frame, t.end_frame()))
}

/// Whether spans are in increasing order, don't overlap and aren't empty (8.4.10 item 2).
fn exact(spans: &[(u32, u32)]) -> bool {
    spans.iter().all(|&(s, e)| s < e) && spans.windows(2).all(|w| w[0].1 <= w[1].0)
}

/// A program: a program start at `start`, then each span's track start, and its track
/// stop when it is the last span or the next one starts later (8.4.10).
fn program(start: u32, spans: &[(u32, u32)]) -> Vec<Marker> {
    let mut m = vec![Marker::new(start, MarkerKind::ProgramStart)];
    for (i, &(s, e)) in spans.iter().enumerate() {
        m.push(Marker::new(s, MarkerKind::TrackStart));
        if spans.get(i + 1).is_none_or(|&(next, _)| e < next) {
            m.push(Marker::new(e, MarkerKind::TrackStop));
        }
    }
    m
}

/// The markers 8.4.10 gives a file of `[a, b)` with source track `source`.
pub(super) fn from_toc(
    area: &Area,
    a: u32,
    b: u32,
    source: Option<&Track>,
    options: &OutputOptions,
) -> Vec<Marker> {
    if let Some(t) = source {
        // Item 3: a one-track program, when the writer chooses one (W28).
        let (s, e) = (a.max(t.start_frame), b.min(t.end_frame()));
        return if options.dsdiff_source_track_markers && s < e { program(a, &[(s, e)]) } else { Vec::new() };
    }
    let clipped: Vec<(u32, u32)> =
        spans(area).map(|(_, s, e)| (a.max(s), b.min(e))).filter(|&(s, e)| s < e).collect();
    let play = (a, b) == (0, area.playtime_frames());
    if !(play || options.dsdiff_clipped_program) || clipped.is_empty() {
        return Vec::new();
    }
    // For the play range the clipped tracks are the tracks themselves (item 4), and both
    // must meet item 2; so must the unclipped tracks, since an empty track clips away.
    let all: Vec<(u32, u32)> = spans(area).map(|(_, s, e)| (s, e)).collect();
    if play && !exact(&all) || !exact(&clipped) {
        return Vec::new();
    }
    program(a, &clipped)
}

/// The checks of 8.4.11 for an edited master of `[a, b)`.
pub(super) fn check_edited_master(area: &Area, a: u32, b: u32) -> Result<(), Error> {
    let refuse = |c| Err(Error::OutputRefused(OutputRefusal::NotEditedMaster(c)));
    if (a, b) != (0, area.playtime_frames()) {
        return refuse(EditedMasterCheck::Range);
    }
    if matches!(area.loudspeaker_layout(), LoudspeakerLayout::Unspecified { .. }) {
        return refuse(EditedMasterCheck::Layout);
    }
    if !(1..=255).contains(&area.tracks.len()) {
        return refuse(EditedMasterCheck::TrackCount);
    }
    if let Some(t) = area.tracks.iter().find(|t| t.duration_frames < 75) {
        return refuse(EditedMasterCheck::TrackTooShort { track: t.number });
    }
    let all: Vec<(u32, u32)> = spans(area).map(|(_, s, e)| (s, e)).collect();
    if !exact(&all) {
        return refuse(EditedMasterCheck::TracksNotInOrder);
    }
    // From the program start (frame 0) to the last track stop: at most 255:59:74.
    if all.last().is_some_and(|&(_, e)| e > (255 * 60 + 59) * 75 + 74) {
        return refuse(EditedMasterCheck::ProgramTooLong);
    }
    Ok(())
}

/// Text bytes of a string the caller gives (8.4.1 item 4).
pub(super) fn text_bytes(s: &[u8]) -> Result<(), Error> {
    if s.iter().all(|b| (0x20..=0x7E).contains(b)) {
        Ok(())
    } else {
        Err(Error::InvalidArgument("DSDIFF text is bytes 0x20 to 0x7E"))
    }
}

/// Appends a `MARK` chunk (8.4.7) of `marker` in a file of `[a, ..)` with absolute start
/// frame `start`.
fn mark(out: &mut Vec<u8>, marker: &Marker, a: u32, start: u64, pad_counted: bool) {
    let (h, m, s, samples) = time(start + u64::from(marker.frame - a));
    let mut d = h.to_be_bytes().to_vec();
    d.extend_from_slice(&[m, s]);
    d.extend_from_slice(&samples.to_be_bytes());
    d.extend_from_slice(&0i32.to_be_bytes());
    d.extend_from_slice(&marker.kind.value().to_be_bytes());
    d.extend_from_slice(&0u16.to_be_bytes());
    let flags = if marker.kind == MarkerKind::TrackStart { marker.track_flags } else { 0 };
    d.extend_from_slice(&flags.to_be_bytes());
    d.extend_from_slice(&(marker.text.len() as u32).to_be_bytes());
    d.extend_from_slice(&marker.text);
    // W22: with the pad counted, `D` is even and 8.4.1 adds no pad of its own.
    if pad_counted && marker.text.len() % 2 == 1 {
        d.push(0);
    }
    chunk(out, b"MARK", &d);
}

/// The `DIIN` chunk (8.4.8): `EMID`, then the markers, in the table's order.
pub(super) fn diin(layout: &Layout, markers: &[Marker], options: &OutputOptions) -> Result<Vec<u8>, Error> {
    text_bytes(options.dsdiff_edited_master_id.as_bytes())?;
    let a = layout.first;
    let start = u64::from(options.dsdiff_absolute_start.unwrap_or(0));
    let mut data = Vec::new();
    chunk(&mut data, b"EMID", options.dsdiff_edited_master_id.as_bytes());
    for m in markers {
        if m.frame < layout.first || m.frame > layout.end {
            return Err(Error::InvalidArgument("a marker lies outside the range"));
        }
        text_bytes(&m.text)?;
        debug_assert!(start + u64::from(layout.end - a) <= MAX_TIME_FRAMES);
        mark(&mut data, m, a, start, options.dsdiff_text_pad_counted);
    }
    let mut out = Vec::new();
    chunk(&mut out, b"DIIN", &data);
    Ok(out)
}
