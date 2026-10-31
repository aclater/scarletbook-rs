// SPDX-License-Identifier: MIT OR Apache-2.0

//! Harness shared by the cargo-fuzz targets (`fuzz/`) and the regression tests, so that
//! a minimised crash runs the exact same code in both.

#![allow(dead_code, clippy::panic, clippy::unwrap_used)]

use std::io::{self, Read, Seek, SeekFrom};

use scarletbook::audio::{FRAME_BYTES_PER_CHANNEL, MAX_FRAME_BYTES};
use scarletbook::dst::FL;
use scarletbook::{AreaIndex, Demuxer, Disc, DstDecoder, FrameKind};

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
/// is the frame. For one input in eight (chosen by the frame's last byte, so runs stay
/// reproducible) the frame is decoded a second time with the same decoder and the results
/// must agree; decoding every input twice halved the fuzzer's throughput.
pub fn dst(data: &[u8]) {
    let Some((&c, frame)) = data.split_first() else { return };
    let channels = c % 6 + 1;
    let mut decoder = DstDecoder::new();
    let mut a = vec![0u8; usize::from(channels) * FL];
    let ra = decoder.decode(frame, channels, &mut a);
    if frame.last().is_some_and(|&b| b % 8 == 0) {
        let mut b = vec![1u8; usize::from(channels) * FL];
        let rb = decoder.decode(frame, channels, &mut b);
        assert_eq!(ra, rb, "decoding is not deterministic");
        if ra.is_ok() {
            assert!(a == b, "decoded output is not deterministic");
        }
    }
}

/// Audio sectors in the area of [`seek_index`].
const SEEK_SECTORS: u32 = 64;

/// A 2-channel area of [`SEEK_SECTORS`] sectors from LSN 0 holding DST frames of 4
/// sectors, a malformed sector, a frame held over 37 sectors without packets and a frame
/// with the largest timecode; and its image.
pub fn seek_area() -> (scarletbook::Image<std::io::Cursor<Vec<u8>>>, scarletbook::Area) {
    use scarletbook::toc::{AreaKind, AreaText, Timecode, Version};
    let mut bytes = vec![0u8; SEEK_SECTORS as usize * 2048];
    for (i, s) in bytes.chunks_exact_mut(2048).enumerate() {
        let packet = |start: bool, len: u16| ((u16::from(start) << 15) | (2 << 11) | len).to_be_bytes();
        let start = |s: &mut [u8], n: u32, count: u8| {
            s[0] = 1 | (1 << 2) | (1 << 5);
            s[1..3].copy_from_slice(&packet(true, 500));
            s[3..6].copy_from_slice(&[(n / 4500) as u8, (n / 75 % 60) as u8, (n % 75) as u8]);
            s[6] = count << 2;
        };
        match i {
            // One packet past the end of the sector (specification 4, 4.4.3 item 1).
            10 => {
                s[0] = 1 << 5;
                s[1..3].copy_from_slice(&packet(false, 2047));
            }
            // DST frames of 4 sectors, numbered i / 4.
            0..20 if i % 4 == 0 => start(s, i as u32 / 4, 4),
            // Frame 5 from sector 20, held over 37 sectors without packets: longer than
            // REPLAY_SECTORS, so a checkpoint inside it stores its bytes.
            20 => start(s, 5, 3),
            21..58 => s[0] = 1,
            // The frame with the largest timecode.
            60 => start(s, 1_166_880, 4),
            _ => {
                s[0] = 1 | (1 << 5);
                s[1..3].copy_from_slice(&packet(false, 500));
            }
        }
    }
    let image =
        scarletbook::Image::with_format(std::io::Cursor::new(bytes), scarletbook::SectorFormat::Plain)
            .unwrap();
    let area = scarletbook::Area {
        slot: scarletbook::Slot::TwoChannel,
        kind: AreaKind::TwoChannel,
        version: Version { major: 1, minor: 20 },
        toc_lsn: 0,
        toc_sectors: 0,
        max_byte_rate: 0,
        sample_frequency_code: 4,
        frame_format: 0,
        channel_count: 2,
        area_config: 0,
        max_available_channels: 2,
        area_mute_flags: 0,
        track_attribute: 0,
        playtime: Timecode { minutes: 0, seconds: 0, frames: 20 },
        track_offset: 0,
        audio_start_lsn: 0,
        audio_end_lsn: SEEK_SECTORS - 1,
        text_channel_count: 0,
        locales: Vec::new(),
        text_charset: 0,
        text: AreaText::default(),
        tracks: Vec::new(),
    };
    (image, area)
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// Seeds for [`seek_index`]: complete indexes of [`seek_area`] at a few intervals, and a
/// partial one, without their checksums.
pub fn seek_index_seeds() -> Vec<Vec<u8>> {
    let (mut image, area) = seek_area();
    let n = image.sector_count();
    let mut out = Vec::new();
    for (k, steps) in [(1, None), (4, None), (7, Some(1)), (64, None)] {
        let options = scarletbook::IndexOptions::new().with_interval(k);
        let mut index = AreaIndex::new(&area, n, options).unwrap();
        match steps {
            Some(sectors) => {
                index.extend(&mut image, sectors * 20).unwrap();
            }
            None => while !index.extend(&mut image, 1000).unwrap() {},
        }
        let mut bytes = index.to_bytes();
        bytes.truncate(bytes.len() - 8);
        out.push(bytes);
    }
    out
}

/// A saved seek index from untrusted bytes, for the area of [`seek_area`]: the input as
/// it is, and with its checksum appended, so that the fuzzer gets past the checksum. A
/// loaded index takes memory proportional to the input, saves to the same bytes, and
/// cursors started from it and its extension read without panicking.
pub fn seek_index(data: &[u8]) {
    let (mut image, area) = seek_area();
    let n = image.sector_count();
    let mut summed = data.to_vec();
    summed.extend_from_slice(&fnv(data).to_le_bytes());
    for bytes in [data, &summed[..]] {
        // Within a small budget, loading drops stored frames; it must not fail otherwise.
        if let Ok(small) = AreaIndex::from_bytes(&area, n, bytes, 600) {
            assert!(small.options().inline_budget <= 600);
        }
        let Ok(index) = AreaIndex::from_bytes(&area, n, bytes, usize::MAX) else { continue };
        assert!(index.memory() <= 1024 + 2 * bytes.len(), "{} for {} bytes", index.memory(), bytes.len());
        assert_eq!(index.to_bytes(), bytes, "saving a loaded index changes it");
        let p = area.playtime_frames();
        for first in [0, p / 2, p - 1] {
            let mut cursor = scarletbook::TrackCursor::for_range_indexed(&area, first, p, n, &index).unwrap();
            while let Some(Ok(_)) = cursor.next_frame(&mut image) {}
            let mut copy = index.clone();
            let _ = copy.absorb(&mut cursor);
        }
        let mut copy = index.clone();
        let _ = copy.extend(&mut image, 1000);
        let mut merged = index.clone();
        let _ = merged.merge(&copy);
    }
}
