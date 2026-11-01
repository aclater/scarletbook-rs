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

/// `bytes` of a saved seek index with their checksum appended.
pub fn with_checksum(bytes: &[u8]) -> Vec<u8> {
    let mut summed = bytes.to_vec();
    summed.extend_from_slice(&fnv(bytes).to_le_bytes());
    summed
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
        if k == 4 {
            // Seek hints (format version 2): those of a read through the index.
            let p = area.playtime_frames();
            let mut cursor = scarletbook::TrackCursor::for_range_indexed(&area, 0, p, n, &index).unwrap();
            while let Some(Ok(_)) = cursor.next_frame(&mut image) {}
            index.absorb(&mut cursor).unwrap();
        }
        let mut bytes = index.to_bytes();
        bytes.truncate(bytes.len() - 8);
        out.push(bytes);
    }
    // Format version 3 (#124): the first version 2 seed with a TOC fingerprint and one saved
    // confirmation, at the area's first sector.
    if let Some(v2) = out.iter().find(|b| b[8] == 2).cloned() {
        let mut sum = v2.clone();
        sum.extend_from_slice(&fnv(&v2).to_le_bytes());
        let hints = AreaIndex::from_bytes(&area, n, &sum, usize::MAX).unwrap().hints();
        let mut v3 = v2;
        v3[8] = 3;
        v3[14] |= 2;
        let at = v3.len() - 8 * hints - 24;
        v3[at..at + 8].copy_from_slice(&1u64.to_le_bytes());
        v3.extend_from_slice(&1u32.to_le_bytes());
        v3.extend_from_slice(&0u32.to_le_bytes());
        v3.extend_from_slice(&area.audio_start_lsn.to_le_bytes());
        v3.extend_from_slice(&0u32.to_le_bytes());
        v3.extend_from_slice(&7u64.to_le_bytes());
        out.push(v3);
    }
    out
}

/// A saved seek index from untrusted bytes, for the area of [`seek_area`]: the input as
/// it is, and with its checksum appended, so that the fuzzer gets past the checksum. A
/// loaded index takes memory proportional to the input, saves to the same bytes, and
/// cursors started from it (exact and speculative, from its seek hints) and its extension
/// read without panicking. The same bytes are then loaded for the disc of [`seek_disc`]
/// (see [`seek_index_disc`]).
pub fn seek_index(data: &[u8]) {
    let (mut image, area) = seek_area();
    let n = image.sector_count();
    let summed = with_checksum(data);
    for bytes in [data, &summed[..]] {
        // Within a small budget, loading drops stored frames; it must not fail otherwise.
        if let Ok(small) = AreaIndex::from_bytes(&area, n, bytes, 600) {
            assert!(small.options().inline_budget <= 600);
        }
        seek_index_disc(bytes);
        let Ok(index) = AreaIndex::from_bytes(&area, n, bytes, usize::MAX) else { continue };
        assert!(index.memory() <= 1024 + 2 * bytes.len(), "{} for {} bytes", index.memory(), bytes.len());
        assert_eq!(index.to_bytes(), bytes, "saving a loaded index changes it");
        // Without its checkpoints, it saves and loads again to the same bytes.
        let light = index.without_checkpoints().to_bytes();
        let reloaded = AreaIndex::from_bytes(&area, n, &light, usize::MAX).unwrap();
        assert_eq!(reloaded.to_bytes(), light, "an index without checkpoints changes when loaded");
        let p = area.playtime_frames();
        let mut copy = index.clone();
        for first in [0, p / 2, p - 1] {
            let mut cursor = scarletbook::TrackCursor::for_range_indexed(&area, first, p, n, &index).unwrap();
            while let Some(Ok(_)) = cursor.next_frame(&mut image) {}
            let _ = copy.absorb(&mut cursor);
        }
        // Speculative cursors use its seek hints (and, unbound to a disc, never its saved
        // confirmations); their readings are checked by the extension below.
        for (first, gap) in [(p / 2, 0), (p - 1, 4)] {
            let options =
                scarletbook::SpeculativeOptions::new().with_min_gap_sectors(gap).with_probe_sectors(2);
            let mut cursor =
                scarletbook::TrackCursor::for_range_speculative(&area, first, p, n, Some(&index), options)
                    .unwrap();
            while let Some(Ok(_)) = cursor.next_frame(&mut image) {}
            let _ = copy.absorb(&mut cursor);
        }
        let _ = copy.extend(&mut image, 1000);
        let _ = copy.verify(&mut image);
        let _ = copy.take_events();
        let mut merged = index.clone();
        let _ = merged.merge(&copy);
    }
}

/// DST frames of the area of [`seek_disc`], 4 sectors each.
const DISC_FRAMES: u32 = 96;

/// The frame ranges [`seek_index_disc`] reads.
const DISC_RANGES: [(u32, u32); 4] = [(60, 64), (30, 33), (88, 96), (45, 47)];

/// A disc with a real TOC (so that it binds a seek index's TOC fingerprint) holding one
/// 2-channel area of [`DISC_FRAMES`] DST frames of 4 sectors each, with a malformed
/// sector inside frame 10; and the frames of each of [`DISC_RANGES`] read without an
/// index. Built once.
pub struct SeekDisc {
    pub image: Vec<u8>,
    pub area: scarletbook::Area,
    pub sectors: u64,
    pub expected: Vec<Vec<scarletbook::TrackFrame>>,
}

/// The disc of [`SeekDisc`].
pub fn seek_disc() -> &'static SeekDisc {
    static DISC: std::sync::OnceLock<SeekDisc> = std::sync::OnceLock::new();
    DISC.get_or_init(|| {
        const S: usize = 2048;
        const AREA_TOC: usize = 540;
        const AUDIO: usize = AREA_TOC + 6;
        let sectors = DISC_FRAMES as usize * 4;
        let mut img = vec![0u8; (AUDIO + sectors) * S];
        let put = |img: &mut [u8], at: usize, b: &[u8]| img[at..at + b.len()].copy_from_slice(b);
        let tc = |n: u32| [(n / 4500) as u8, (n / 75 % 60) as u8, (n % 75) as u8];
        // Master TOC, its text, the manufacturer sector and the two copies.
        let m = 510 * S;
        put(&mut img, m, b"SACDMTOC");
        put(&mut img, m + 8, &[1, 20]);
        put(&mut img, m + 16, &[0, 1, 0, 1]);
        put(&mut img, m + 24, b"CAT-1");
        put(&mut img, m + 64, &(AREA_TOC as u32).to_be_bytes());
        put(&mut img, m + 84, &6u16.to_be_bytes());
        img[m + 128] = 1;
        put(&mut img, m + 136, b"en\x02");
        put(&mut img, m + S, b"SACDText");
        put(&mut img, m + 9 * S, b"SACD_Man");
        let master = img[m..m + 10 * S].to_vec();
        for copy in [520, 530] {
            put(&mut img, copy * S, &master);
        }
        // The area TOC: one track over the whole area.
        let a = AREA_TOC * S;
        put(&mut img, a, b"TWOCHTOC");
        put(&mut img, a + 8, &[1, 20]);
        img[a + 20] = 4;
        img[a + 32] = 2;
        put(&mut img, a + 64, &tc(DISC_FRAMES));
        img[a + 69] = 1;
        put(&mut img, a + 72, &(AUDIO as u32).to_be_bytes());
        put(&mut img, a + 76, &((AUDIO + sectors - 1) as u32).to_be_bytes());
        img[a + 80] = 1;
        put(&mut img, a + 88, b"en\x02");
        put(&mut img, a + S, b"SACDTTxt");
        put(&mut img, a + 2 * S, b"SACD_IGL");
        put(&mut img, a + 4 * S, b"SACDTRL1");
        put(&mut img, a + 5 * S, b"SACDTRL2");
        put(&mut img, a + 4 * S + 8, &(AUDIO as u32).to_be_bytes());
        put(&mut img, a + 4 * S + 1028, &1u32.to_be_bytes());
        put(&mut img, a + 5 * S + 1028, &tc(DISC_FRAMES));
        // The audio sectors, each filled with its own byte.
        let packet = |start: bool, len: u16| ((u16::from(start) << 15) | (2 << 11) | len).to_be_bytes();
        for (i, s) in img[AUDIO * S..].chunks_exact_mut(S).enumerate() {
            s.fill(i as u8);
            if i == 42 {
                s[0] = 1 << 5;
                s[1..3].copy_from_slice(&packet(false, 2047));
            } else if i % 4 == 0 {
                s[0] = 1 | (1 << 2) | (1 << 5);
                s[1..3].copy_from_slice(&packet(true, 500));
                s[3..6].copy_from_slice(&tc(i as u32 / 4));
                s[6] = 4 << 2;
            } else {
                s[0] = 1 | (1 << 5);
                s[1..3].copy_from_slice(&packet(false, 500));
            }
        }
        let mut disc = Disc::open(std::io::Cursor::new(&img[..])).unwrap();
        let area = disc.toc().areas[0].clone();
        let n = disc.toc().sectors;
        let expected = DISC_RANGES
            .iter()
            .map(|&(f, e)| disc.range_frames(area.slot, f, e).unwrap().map(Result::unwrap).collect())
            .collect();
        SeekDisc { image: img, area, sectors: n, expected }
    })
}

/// Speculative options that read provisionally in the disc of [`seek_disc`].
fn disc_speculative() -> scarletbook::SeekPolicy {
    scarletbook::SeekPolicy::Speculative(scarletbook::SpeculativeOptions::new().with_min_gap_sectors(8))
}

/// Reads `[first, end)` of `disc`, returning its frames and its cursor.
fn disc_read<R: Read + Seek>(
    disc: &mut Disc<R>,
    (first, end): (u32, u32),
) -> (Vec<scarletbook::TrackFrame>, scarletbook::TrackCursor) {
    let c = disc.range_frames_cursor(scarletbook::Slot::TwoChannel, first, end).unwrap();
    let mut it = disc.read_frames(c);
    let frames = it.by_ref().map_while(Result::ok).collect();
    (frames, it.into_cursor())
}

/// A saved seek index from untrusted bytes, for the disc of [`seek_disc`]: kept by the
/// disc ([`Disc::set_seek_index`], which binds the TOC fingerprint so that saved
/// confirmations are used), it reads two of [`DISC_RANGES`] provisionally (chosen by the
/// input's last byte), then verifies them with a pass over the whole area. The frames
/// read are numbered within their range and increasing; and when the index held only seek
/// hints (which are checked, unlike saved checkpoints and confirmations, which are trusted
/// for an image whose TOC fingerprint matches), they are the frames read without an index,
/// from the first exact position of the reading's verdict on.
pub fn seek_index_disc(bytes: &[u8]) {
    let d = seek_disc();
    let Ok(index) = AreaIndex::from_bytes(&d.area, d.sectors, bytes, usize::MAX) else { return };
    let checked = index.checkpoints() == 0 && index.confirmations() == 0 && !index.is_complete();
    let slot = d.area.slot;
    let mut disc = Disc::open(std::io::Cursor::new(&d.image[..])).unwrap();
    if disc.set_seek_index(slot, index).is_err() {
        return;
    }
    disc.set_seek_policy(slot, disc_speculative()).unwrap();
    let pick = usize::from(bytes.last().copied().unwrap_or(0));
    let mut reads = Vec::new();
    for k in [pick % 4, (pick / 4 + 1 + pick % 4) % 4] {
        let range = DISC_RANGES[k];
        let (frames, cursor) = disc_read(&mut disc, range);
        assert!(frames.iter().all(|f| (range.0..range.1).contains(&f.number)), "a frame outside {range:?}");
        assert!(frames.windows(2).all(|w| w[0].number < w[1].number), "frames not increasing");
        reads.push((k, frames, cursor));
    }
    while !disc.verify_speculation(slot, 1000).unwrap() {}
    let _ = disc.take_seek_events(slot);
    let kept = disc.seek_index(slot).unwrap();
    assert!(kept.memory() <= 1 << 20, "{} bytes", kept.memory());
    let saved = kept.to_bytes();
    assert_eq!(AreaIndex::from_bytes(&d.area, d.sectors, &saved, usize::MAX).unwrap().to_bytes(), saved);
    if !checked {
        return;
    }
    for (k, frames, cursor) in reads {
        let exact_from = match cursor.speculation() {
            scarletbook::Speculation::Exact | scarletbook::Speculation::Confirmed { .. } => 0,
            scarletbook::Speculation::Refuted { exact_from, .. } => exact_from,
            s => panic!("{s:?} after the whole area was verified"),
        };
        let want: Vec<_> = d.expected[k].iter().filter(|f| f.number >= exact_from).collect();
        let got: Vec<_> = frames.iter().filter(|f| f.number >= exact_from).collect();
        assert!(got == want, "{:?} differs from a read without an index", DISC_RANGES[k]);
    }
}

/// Seeds for [`seek_index_disc`]: what a disc of [`seek_disc`] saves after provisional
/// reads of [`DISC_RANGES`]: its seek hints alone (format version 2, before verifying), its
/// hints and confirmations (version 3), and the whole index; without their checksums.
pub fn seek_index_disc_seeds() -> Vec<Vec<u8>> {
    let d = seek_disc();
    let slot = d.area.slot;
    let mut disc = Disc::open(std::io::Cursor::new(&d.image[..])).unwrap();
    disc.enable_seek_index(slot, scarletbook::IndexOptions::new().with_interval(4)).unwrap();
    disc.set_seek_policy(slot, disc_speculative()).unwrap();
    for r in DISC_RANGES {
        disc_read(&mut disc, r);
    }
    let hints = disc.seek_index(slot).unwrap().without_checkpoints();
    while !disc.verify_speculation(slot, 1000).unwrap() {}
    let index = disc.seek_index(slot).unwrap();
    let mut out = Vec::new();
    for i in [hints, index.without_checkpoints(), index] {
        let mut bytes = i.to_bytes();
        bytes.truncate(bytes.len() - 8);
        out.push(bytes);
    }
    out
}
