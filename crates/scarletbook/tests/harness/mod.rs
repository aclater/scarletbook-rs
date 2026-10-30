// SPDX-License-Identifier: MIT OR Apache-2.0

//! Harness shared by the cargo-fuzz targets (`fuzz/`) and the regression tests, so that
//! a minimised crash runs the exact same code in both.

#![allow(dead_code, clippy::panic, clippy::unwrap_used)]

use std::io::{self, Read, Seek, SeekFrom};

use scarletbook::audio::{FRAME_BYTES_PER_CHANNEL, MAX_FRAME_BYTES};
use scarletbook::dst::FL;
use scarletbook::{Demuxer, Disc, DstDecoder, FrameKind};

/// Bytes of 510 empty plain sectors, prepended to fuzz input so that the input starts at
/// the master TOC.
pub const PREFIX: u64 = 510 * 2048;

/// A reader over `PREFIX` zero bytes followed by `data`, without allocating the prefix.
#[derive(Debug)]
pub struct Prefixed<'a> {
    data: &'a [u8],
    pos: u64,
}

impl<'a> Prefixed<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Prefixed { data, pos: 0 }
    }

    fn len(&self) -> u64 {
        PREFIX + self.data.len() as u64
    }
}

impl Read for Prefixed<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = if self.pos < PREFIX {
            let n = buf.len().min((PREFIX - self.pos) as usize);
            buf[..n].fill(0);
            n
        } else {
            let at = (self.pos - PREFIX) as usize;
            let src = self.data.get(at..).unwrap_or(&[]);
            let n = buf.len().min(src.len());
            buf[..n].copy_from_slice(&src[..n]);
            n
        };
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for Prefixed<'_> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let new = match to {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::End(d) => self.len().checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        self.pos = new.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bad seek"))?;
        Ok(self.pos)
    }
}

/// Parses the TOC of `PREFIX` zeros followed by `data` and checks the TOC invariants.
pub fn toc(data: &[u8]) {
    if let Ok(disc) = Disc::open(Prefixed::new(data)) {
        check_toc(&disc);
    }
}

/// Invariants every accepted TOC satisfies, whatever the input.
pub fn check_toc<R>(disc: &Disc<R>)
where
    R: Read + Seek,
{
    let toc = disc.toc();
    assert!(!toc.areas.is_empty() && toc.areas.len() <= 2);
    if toc.areas.len() == 2 {
        assert_ne!(toc.areas[0].slot, toc.areas[1].slot);
    }
    assert!(toc.master.version.major <= 1);
    assert!(toc.master.locales.len() <= 8);
    let text_ok = |s: &str| !s.is_empty() && s.chars().all(|c| c >= ' ');
    assert!(toc.master.text.iter().all(|(_, s)| text_ok(s)));
    for area in &toc.areas {
        assert!(area.toc_sectors >= 1 && area.toc_sectors <= 128);
        assert!(area.audio_start_lsn <= area.audio_end_lsn);
        assert!(area.locales.len() <= 10);
        assert!(area.text.iter().all(|(_, s)| text_ok(s)));
        let p = area.playtime_frames();
        for (i, t) in area.tracks.iter().enumerate() {
            assert_eq!(usize::from(t.number), i + 1);
            assert!(t.start_frame + t.duration_frames <= p);
            assert!(t.text.iter().all(|(_, s)| text_ok(s)));
            assert!(t.isrc.as_ref().is_none_or(|s| !s.is_empty() && s.chars().count() <= 12));
        }
    }
}

/// Frame demultiplexing: byte 0 is the area's channel count (any value), the rest is a
/// run of 2048-byte audio sectors (a trailing partial sector is ignored).
pub fn demux(data: &[u8]) {
    let Some((&channels, sectors)) = data.split_first() else { return };
    let mut d = Demuxer::new(channels);
    let mut last: Option<u32> = None;
    let mut check = |d: &mut Demuxer| {
        while let Some(f) = d.pop() {
            assert!(last.is_none_or(|l| f.number > l), "timeline not increasing");
            last = Some(f.number);
            assert!(!f.data.is_empty() && f.data.len() <= MAX_FRAME_BYTES);
            if f.kind == FrameKind::Dsd {
                assert_eq!(f.data.len(), usize::from(channels) * FRAME_BYTES_PER_CHANNEL);
            }
        }
    };
    for s in sectors.chunks_exact(2048) {
        let mut sector = [0u8; 2048];
        sector.copy_from_slice(s);
        d.push_sector(&sector);
        check(&mut d);
    }
    d.finish();
    check(&mut d);
}

/// Single-frame DST decoding: byte 0 picks the channel count (`byte % 6 + 1`), the rest
/// is the frame. Decoding the same frame twice with one decoder gives the same result.
pub fn dst(data: &[u8]) {
    let Some((&c, frame)) = data.split_first() else { return };
    let channels = c % 6 + 1;
    let mut decoder = DstDecoder::new();
    let mut a = vec![0u8; usize::from(channels) * FL];
    let mut b = vec![1u8; usize::from(channels) * FL];
    let ra = decoder.decode(frame, channels, &mut a);
    let rb = decoder.decode(frame, channels, &mut b);
    assert_eq!(ra, rb, "decoding is not deterministic");
    if ra.is_ok() {
        assert!(a == b, "decoded output is not deterministic");
    }
}
