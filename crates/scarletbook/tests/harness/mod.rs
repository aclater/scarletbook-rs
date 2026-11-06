// SPDX-License-Identifier: MIT OR Apache-2.0

//! Harness shared by the cargo-fuzz targets (`fuzz/`) and the regression tests, so that
//! a minimised crash runs the exact same code in both.

#![allow(dead_code, clippy::panic, clippy::unwrap_used)]

use std::io::{self, Read, Seek, SeekFrom};

use scarletbook::audio::FRAME_BYTES_PER_CHANNEL as FL;
use scarletbook::audio::{FRAME_BYTES_PER_CHANNEL, MAX_FRAME_BYTES};
use scarletbook::{AreaIndex, Demuxer, Disc, DstDecoder, FrameKind};
use scarletbook_pcm::{ConvertStats, Converter, Cutoff, FrameRun, Gain, PcmConfig, PcmError, PcmRate};

pub mod output;

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

/// Parses the TOC of `PREFIX` zeros followed by `data` and checks the TOC invariants,
/// then the output files of a range of it that the input's last bytes choose
/// ([`output::CONTROL`]).
pub fn toc(data: &[u8]) {
    if let Ok(mut disc) = Disc::open(Prefixed::new(data)) {
        check_toc(&disc);
        output::check_outputs(&mut disc, data);
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
/// run of 2048-byte audio sectors (a trailing partial sector is ignored). For the inputs
/// that [`pcm_case`] selects (one in 4096), the same bytes are also converted to PCM
/// ([`pcm`]).
pub fn demux(data: &[u8]) {
    demux_case(data);
}

/// [`demux`], returning the PCM case it converted, if any (for the fuzz target's C checks).
pub fn demux_case(data: &[u8]) -> Option<PcmCase<'_>> {
    let case = pcm_case(data);
    if let Some(c) = &case {
        pcm(c);
    }
    demux_frames(data);
    case
}

fn demux_frames(data: &[u8]) {
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

/// One `demux` input in this many is converted to PCM ([`pcm_case`]). Under the fuzzer's
/// instrumentation (compare tracing in the filter loops) a conversion costs milliseconds per
/// frame and channel, thousands of times a demultiplexing. The inputs are chosen by a hash
/// of all their bytes, so that a mutation of a converted input is converted no more often
/// than any other: selected by a byte, they took the fuzzer's time (233 runs/s instead of
/// 28 000).
pub const PCM_ONE_IN: u64 = 4096;

/// Frame-channels (frames x channels) of a PCM case: [`PCM_SHORT`] at most, or for one
/// channel and one case in two, [`PCM_LONG`] frames, to cross the C interface's chunks of
/// 8 frames.
const PCM_SHORT: usize = 3;
const PCM_LONG: usize = 11;

/// A DSD-to-PCM conversion taken from a `demux` input: the arguments of
/// `sb_pcm_converter_new` (some invalid) and a run of frames with its neighbours.
#[derive(Debug)]
pub struct PcmCase<'a> {
    /// Byte 0 of the input as it is: 1 to 6 is a valid channel count.
    pub channels_raw: u8,
    /// The channel count of the frames, `byte 0 % 6 + 1`.
    pub channels: u8,
    /// 88 200, 176 400, 352 800, or 44 100 (invalid).
    pub rate_hz: u32,
    /// `SB_PCM_CUTOFF_*` 0 to 2, or 3 (invalid).
    pub cutoff: u32,
    /// The gain in dB, sometimes outside -120 to +24 or not finite.
    pub gain_db: f64,
    pub before: Option<&'a [u8]>,
    pub frames: &'a [u8],
    pub after: Option<&'a [u8]>,
    /// Where [`pcm`] splits the run in two, in frames (0 for no split).
    pub split: usize,
}

/// The PCM case of one `demux` input in [`PCM_ONE_IN`] (by a hash of the input, and once
/// per input): the sector bytes from byte 1 on taken as whole frames of `byte 0 % 6 + 1`
/// channels, and its last three bytes a shape byte (run length, split), a gain byte and a
/// configuration byte (rate, cutoff, neighbours). `None` for other inputs, and when the
/// input holds no whole frame.
pub fn pcm_case(data: &[u8]) -> Option<PcmCase<'_>> {
    let channels_raw = *data.first()?;
    let channels = channels_raw % 6 + 1;
    let frame = usize::from(channels) * scarletbook_pcm::FRAME_BYTES_PER_CHANNEL;
    if data.len() < 1 + frame || !pcm_selected(data) {
        return None;
    }
    let [shape, b, a] = data[data.len() - 3..] else { return None };
    let bytes = &data[1..];
    let most =
        if channels == 1 && shape & 1 != 0 { PCM_LONG } else { (PCM_SHORT / usize::from(channels)).max(1) };
    let count = (bytes.len() / frame).min(most);
    if count == 0 {
        return None;
    }
    let all: Vec<&[u8]> = bytes.chunks_exact(frame).take(count).collect();
    // Neighbours come from the frames themselves, when there are frames to spare.
    let (mut lo, mut hi) = (0, count);
    let before = (a & 0x10 != 0 && hi - lo > 1).then(|| {
        lo += 1;
        all[0]
    });
    let after = (a & 0x20 != 0 && hi - lo > 1).then(|| {
        hi -= 1;
        all[hi]
    });
    let gain_db = match b {
        0..=239 => -120.0 + f64::from(b) * 144.0 / 239.0,
        240 => 0.0,
        241 => f64::NAN,
        242 => f64::INFINITY,
        243 => f64::NEG_INFINITY,
        244 => 24.000_000_001,
        245 => -120.000_000_001,
        246 => 24.0,
        247 => -120.0,
        _ => f64::from(b) - 200.0,
    };
    Some(PcmCase {
        channels_raw,
        channels,
        rate_hz: [88_200, 176_400, 352_800, 44_100][usize::from(a & 3)],
        cutoff: u32::from(a >> 2 & 3),
        gain_db,
        before,
        frames: &bytes[lo * frame..hi * frame],
        after,
        split: usize::from(shape >> 1) % (hi - lo),
    })
}

/// Seeds of `demux` inputs that [`pcm_case`] selects, for 1, 2 and 6 channels, each with
/// another rate, cutoff, gain, neighbour case and run length.
pub fn pcm_seeds() -> Vec<Vec<u8>> {
    let mut x = 0x2545_f491_4f6c_dd1du64;
    // (channels byte, frames, shape, gain, configuration): 176.4 kHz Default, 11 frames
    // with both neighbours, split; 88.2 kHz Low, 3 frames, before only, split; 352.8 kHz
    // Wide, one frame and no neighbours.
    [(0u8, PCM_LONG, 0x0Bu8, 120u8, 0x31u8), (1, 3, 0x02, 200, 0x14), (5, 1, 0x00, 60, 0x0A)]
        .into_iter()
        .map(|(channels, frames, shape, gain, config)| {
            let mut seed = vec![channels];
            let len = frames * usize::from(channels % 6 + 1) * scarletbook_pcm::FRAME_BYTES_PER_CHANNEL;
            seed.extend((0..len).map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                // Mostly the alternating pattern of DSD silence, with some noise.
                if x % 4 == 0 { x as u8 } else { 0x69 }
            }));
            seed.extend_from_slice(&[shape, gain, config]);
            pcm_select(&mut seed);
            seed
        })
        .collect()
}

/// Whether [`pcm_case`] may take `input`: a hash of all its bytes is below
/// `2^64 / PCM_ONE_IN`. It reads 64 bytes per step into 8 lanes: under the fuzzer's
/// instrumentation, a byte per step took more time than demultiplexing the input.
fn pcm_hash(input: &[u8]) -> Option<u64> {
    const K: u64 = 0x9e37_79b9_7f4a_7c15;
    let blocks = input.chunks_exact(64);
    let mut lanes = [input.len() as u64 ^ fnv(blocks.remainder()); 8];
    for block in blocks {
        let block: &[u8; 64] = block.try_into().unwrap();
        for (i, lane) in lanes.iter_mut().enumerate() {
            let w = u64::from_le_bytes(block[8 * i..8 * i + 8].try_into().unwrap());
            *lane = (*lane ^ w).wrapping_mul(K).rotate_left(31);
        }
    }
    let h = lanes.iter().fold(0, |h: u64, &l| (h ^ l).wrapping_mul(K).rotate_left(27));
    ((h ^ h >> 29).wrapping_mul(K) < u64::MAX / PCM_ONE_IN).then_some(h)
}

/// Whether [`pcm_case`] takes `input`: [`pcm_hash`], and not taken before (the memory is
/// cleared after 2^16 inputs). Many of the fuzzer's mutations of an input full of repeated
/// bytes change nothing, and a converted input was converted again for each of them.
fn pcm_selected(input: &[u8]) -> bool {
    let Some(h) = pcm_hash(input) else { return false };
    static TAKEN: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());
    let mut taken = TAKEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if taken.contains(&h) {
        return false;
    }
    if taken.len() == 1 << 16 {
        taken.clear();
    }
    taken.push(h);
    true
}

/// Changes bytes 1 and 2 of `input` (its first DSD bytes) so that [`pcm_case`] selects it.
pub fn pcm_select(input: &mut [u8]) {
    for t in 0..=u16::MAX {
        input[1..3].copy_from_slice(&t.to_le_bytes());
        if pcm_hash(input).is_some() {
            return;
        }
    }
    panic!("no selected input");
}

/// DSD-to-PCM conversion of a [`PcmCase`]: invalid configurations are refused with their
/// error; a valid one converts the run without panicking, and the whole run gives the same
/// samples and clip counts as each frame converted with its neighbours and as the run split
/// in two (scarletbook-pcm's determinism). Inputs and outputs of the wrong length are
/// refused, with nothing written.
pub fn pcm(case: &PcmCase<'_>) {
    let cutoff = match case.cutoff {
        0 => Cutoff::Standard,
        1 => Cutoff::Low,
        2 => Cutoff::Wide,
        _ => return,
    };
    let Some(rate) = PcmRate::from_hz(case.rate_hz) else { return };
    let gain = match Gain::from_db(case.gain_db) {
        Ok(g) => g,
        Err(e) => {
            assert!(!(Gain::MIN_DB..=Gain::MAX_DB).contains(&case.gain_db), "{} dB refused", case.gain_db);
            assert!(matches!(e, PcmError::Gain(_)));
            Gain::UNITY
        }
    };
    let raw = Converter::new(PcmConfig::new(rate, cutoff, case.channels_raw).with_gain(gain));
    let config = PcmConfig::new(rate, cutoff, case.channels).with_gain(gain);
    let mut conv = match Converter::new(config) {
        Ok(c) => c,
        Err(e) => {
            assert!(
                matches!(e, PcmError::Unsupported { rate: r, cutoff: c, .. } if r == rate && c == cutoff),
                "{e:?}"
            );
            assert!(!cutoff.supports(rate));
            assert!(raw.is_err());
            return;
        }
    };
    if !(1..=6).contains(&case.channels_raw) {
        assert_eq!(raw.unwrap_err(), PcmError::Channels(case.channels_raw));
    }
    let channels = usize::from(case.channels);
    let frame = channels * scarletbook_pcm::FRAME_BYTES_PER_CHANNEL;
    let per_frame = conv.samples_per_frame() * channels;
    let count = case.frames.len() / frame;
    let run = |before, frames, after| FrameRun::new(frames).with_before(before).with_after(after);

    let mut whole = vec![0i32; count * per_frame];
    let stats = conv.convert_run(run(case.before, case.frames, case.after), &mut whole).unwrap();
    let limit = 1 << 23;
    assert!(whole.iter().all(|s| (-limit..limit).contains(s)), "a sample outside 24 bits");
    assert!(stats.clipped[channels..].iter().all(|&c| c == 0));

    // Frame by frame, each with its neighbours (one frame is the whole run).
    let frame_at = |k: usize| &case.frames[k * frame..(k + 1) * frame];
    let mut sum = ConvertStats::default();
    let mut one = vec![0i32; per_frame];
    for k in (0..count).filter(|_| count > 1) {
        let prev = if k == 0 { case.before } else { Some(frame_at(k - 1)) };
        let next = if k + 1 == count { case.after } else { Some(frame_at(k + 1)) };
        let s = conv.convert_frame(prev, frame_at(k), next, &mut one).unwrap();
        assert!(one == whole[k * per_frame..(k + 1) * per_frame], "frame {k} of {count} differs");
        for (t, c) in sum.clipped.iter_mut().zip(s.clipped) {
            *t += c;
        }
    }
    assert!(count == 1 || sum == stats, "clip counts frame by frame");

    // Split in two at `split` frames.
    if case.split > 0 {
        let m = case.split;
        let mut parts = vec![0i32; count * per_frame];
        let (p, q) = parts.split_at_mut(m * per_frame);
        let s1 = conv.convert_run(run(case.before, &case.frames[..m * frame], Some(frame_at(m))), p).unwrap();
        let s2 =
            conv.convert_run(run(Some(frame_at(m - 1)), &case.frames[m * frame..], case.after), q).unwrap();
        assert!(parts == whole, "the run split at frame {m} differs");
        assert_eq!(s1.clipped_total() + s2.clipped_total(), stats.clipped_total());
    }

    // Wrong lengths: refused, nothing written.
    let mut out = vec![7i32; whole.len() + 1];
    let e = conv.convert_run(run(case.before, case.frames, case.after), &mut out).unwrap_err();
    assert!(matches!(e, PcmError::OutputLength { .. }), "{e:?}");
    let e = conv.convert_run(run(case.before, &case.frames[1..], case.after), &mut out[1..]).unwrap_err();
    assert!(matches!(e, PcmError::InputLength { .. }), "{e:?}");
    let short = &frame_at(0)[1..];
    let e = conv.convert_run(run(Some(short), case.frames, case.after), &mut out[1..]).unwrap_err();
    assert!(matches!(e, PcmError::InputLength { .. }), "{e:?}");
    if count > 1 {
        let e = conv.convert_frame(None, case.frames, None, &mut out[..per_frame]).unwrap_err();
        assert!(matches!(e, PcmError::InputLength { .. }), "{e:?}");
    }
    assert!(out.iter().all(|&s| s == 7), "a refused conversion wrote");
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
    // `Area` can't be built by literal outside the crate: start from a parsed one and set
    // every field, so that this area does not depend on the disc it came from.
    static PARSED: std::sync::OnceLock<scarletbook::Area> = std::sync::OnceLock::new();
    let parsed = PARSED.get_or_init(|| {
        let img = seek_disc_image();
        let disc = Disc::open(std::io::Cursor::new(&img[..])).unwrap();
        disc.toc().areas[0].clone()
    });
    let mut area = parsed.clone();
    area.slot = scarletbook::Slot::TwoChannel;
    area.kind = AreaKind::TwoChannel;
    area.version = Version { major: 1, minor: 20 };
    area.toc_lsn = 0;
    area.toc_sectors = 0;
    area.max_byte_rate = 0;
    area.sample_frequency_code = 4;
    area.frame_format = 0;
    area.channel_count = 2;
    area.area_config = 0;
    area.max_available_channels = 2;
    area.area_mute_flags = 0;
    area.track_attribute = 0;
    area.playtime = Timecode { minutes: 0, seconds: 0, frames: 20 };
    area.track_offset = 0;
    area.audio_start_lsn = 0;
    area.audio_end_lsn = SEEK_SECTORS - 1;
    area.text_channel_count = 0;
    area.locales = Vec::new();
    area.text_charset = 0;
    area.text = AreaText::default();
    area.tracks = Vec::new();
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
        let hints = AreaIndex::from_bytes(&area, n, &sum, usize::MAX).unwrap().hint_count();
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

/// The image of [`SeekDisc`].
fn seek_disc_image() -> Vec<u8> {
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
    img
}

/// The disc of [`SeekDisc`].
pub fn seek_disc() -> &'static SeekDisc {
    static DISC: std::sync::OnceLock<SeekDisc> = std::sync::OnceLock::new();
    DISC.get_or_init(|| {
        let img = seek_disc_image();
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
    let checked = index.checkpoint_count() == 0 && index.confirmation_count() == 0 && !index.is_complete();
    let has_checkpoints = index.checkpoint_count() > 0;
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
    // Bytes that load can hold checkpoints of no reading of this image: extending the index
    // refuses them with a typed error, as documented (#253). Without a checkpoint, nothing
    // can mismatch.
    loop {
        match disc.verify_speculation(slot, 1000) {
            Ok(true) => break,
            Ok(false) => {}
            Err(scarletbook::Error::InvalidArgument("seek index does not match the image"))
                if has_checkpoints =>
            {
                break;
            }
            Err(e) => panic!("verifying: {e:?}"),
        }
    }
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
