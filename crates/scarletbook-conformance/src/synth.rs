// SPDX-License-Identifier: MIT OR Apache-2.0

//! Large synthetic disc images for benchmarking (issue #18).
//!
//! The image is written sector by sector, so its size is not limited by memory. Layout:
//!
//! - LSN 510, 520 and 530: three identical copies of the master TOC (specification 1,
//!   1.5.1; specification 2);
//! - from LSN 540, for each area in slot order: TOC-1 (6 sectors: header, `SACDTTxt`,
//!   `SACD_IGL`, `SACDTRL1`, `SACDTRL2`; specification 3), the audio sectors
//!   (specification 4, 4.3), then TOC-2, a byte-identical copy of TOC-1 (the order of real
//!   discs, specification 1, 1.5.2).
//!
//! Frames are packed in order with timecodes increasing by one, one packet per frame
//! piece, at most 7 packets and 7 frame records per sector, a padding packet (type 7)
//! filling the end of a sector where a record fits, and each DST frame's sector count equal
//! to the number of packets that carry it. Plain DSD frames are `C x 4704` bytes of a
//! deterministic pseudo-random signal; DST frames are real coded frames demultiplexed from
//! the conformance corpus and repeated in a cycle.

use std::fs::File;
use std::io::{self, BufReader, Seek, SeekFrom, Write};
use std::path::Path;

use scarletbook::audio::FRAME_BYTES_PER_CHANNEL;
use scarletbook::{Disc, FrameKind, SectorFormat, Slot};

const S: usize = 2048;

/// Sectors of each area TOC copy.
pub const AREA_TOC_SECTORS: u32 = 6;

/// LSN of the first area TOC (specification 1, 1.5).
const FIRST_AREA_LSN: u32 = 540;

/// Largest frame number a timecode can express (specification 0, 0.5).
pub const MAX_FRAMES: u32 = 1_166_880;

/// Largest DST sector count (specification 4, 4.3.3).
const MAX_DST_PACKETS: usize = 31;

/// Frame bytes in a sector holding only a continuation piece: header and one packet record.
const CONTINUATION_BYTES: usize = S - 3;

/// The frame format of the generated areas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Plain DSD frames.
    Dsd,
    /// DST-coded frames.
    Dst,
}

/// What to generate.
#[derive(Clone, Debug)]
pub struct Config {
    /// Duration of each area, in frames (1/75 s).
    pub frames: u32,
    /// Whether to write a 2-channel area (2-channel slot).
    pub two_channel: bool,
    /// Whether to write a multichannel area (multichannel slot).
    pub multichannel: bool,
    /// Channels of the multichannel area: 5 or 6.
    pub mc_channels: u8,
    /// Frame format.
    pub kind: Kind,
    /// Stored sector format.
    pub format: SectorFormat,
    /// Frames per track; the last track takes the rest.
    pub track_frames: u32,
}

/// Real DST frames to repeat, by channel count.
#[derive(Clone, Debug, Default)]
pub struct DstSource {
    /// 2-channel frames.
    pub two: Vec<Vec<u8>>,
    /// 5-channel frames.
    pub five: Vec<Vec<u8>>,
    /// 6-channel frames.
    pub six: Vec<Vec<u8>>,
}

impl DstSource {
    /// The DST frames of `dst-features-2ch-6ch` (2 and 6 channels) and `dst-features-5ch`
    /// (5 channels) in the corpus at `corpus`, keeping only frames that decode.
    ///
    /// # Errors
    ///
    /// If an image cannot be read or yields no usable frame.
    pub fn from_corpus(corpus: &Path) -> Result<Self, String> {
        let images = corpus.join("images");
        let two_six = images.join("dst-features-2ch-6ch.iso");
        let five = images.join("dst-features-5ch.iso");
        let src = DstSource {
            two: area_dst_frames(&two_six, Slot::TwoChannel, 2)?,
            six: area_dst_frames(&two_six, Slot::Multichannel, 6)?,
            five: area_dst_frames(&five, Slot::Multichannel, 5)?,
        };
        Ok(src)
    }

    fn frames(&self, channels: u8) -> &[Vec<u8>] {
        match channels {
            2 => &self.two,
            5 => &self.five,
            _ => &self.six,
        }
    }
}

fn area_dst_frames(path: &Path, slot: Slot, channels: u8) -> Result<Vec<Vec<u8>>, String> {
    let err = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let file = File::open(path).map_err(|e| err(&e))?;
    let mut disc = Disc::open(BufReader::new(file)).map_err(|e| err(&e))?;
    let area = disc.toc().area(slot).ok_or_else(|| err(&"no such area"))?;
    if area.channel_count != channels {
        return Err(err(&format!("expected {channels} channels, found {}", area.channel_count)));
    }
    let mut out = Vec::new();
    let mut pcm = vec![0u8; usize::from(channels) * FRAME_BYTES_PER_CHANNEL];
    for f in disc.area_frames(slot).map_err(|e| err(&e))? {
        let f = f.map_err(|e| err(&e))?;
        if f.kind == FrameKind::Dst && scarletbook::dst::decode(&f.data, channels, &mut pcm).is_ok() {
            out.push(f.data);
        }
    }
    if out.is_empty() {
        return Err(err(&"no decodable DST frame"));
    }
    Ok(out)
}

/// Where one generated area is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AreaLayout {
    /// Its slot.
    pub slot: Slot,
    /// Channel count.
    pub channels: u8,
    /// TOC-1 and TOC-2 LSNs.
    pub toc_lsns: (u32, u32),
    /// First and last audio LSN.
    pub audio: (u32, u32),
    /// Per track: start LSN, length in sectors, start frame, duration in frames.
    pub tracks: Vec<(u32, u32, u32, u32)>,
}

/// What was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// The areas, in slot order.
    pub areas: Vec<AreaLayout>,
    /// Logical sectors in the image.
    pub sectors: u64,
}

fn put(buf: &mut [u8], at: usize, bytes: &[u8]) {
    buf[at..at + bytes.len()].copy_from_slice(bytes);
}

fn put16(buf: &mut [u8], at: usize, v: u16) {
    put(buf, at, &v.to_be_bytes());
}

fn put32(buf: &mut [u8], at: usize, v: u32) {
    put(buf, at, &v.to_be_bytes());
}

/// A timecode whose frame number (specification 0, 0.5 item 2) is `n`, for any
/// `n <= MAX_FRAMES`.
///
/// Up to 255:59:74 the timecode is normalised (0.5 item 3). Beyond it the minutes byte
/// stays at 255 and the seconds and frames bytes carry the rest, which 0.5 item 2 allows
/// (readers accept s >= 60 and f >= 75): that is how a timecode reaches MAX_FRAMES.
fn tc(n: u32) -> [u8; 3] {
    let m = (n / 4500).min(255);
    let s = ((n - 4500 * m) / 75).min(255);
    let f = n - 4500 * m - 75 * s;
    // For n <= MAX_FRAMES every part is at most 255.
    let byte = |v: u32| u8::try_from(v).unwrap_or(u8::MAX);
    [byte(m), byte(s), byte(f)]
}

/// Writes logical sectors in the stored format, tracking the LSN of the position.
struct Out<W> {
    w: W,
    format: SectorFormat,
    lsn: u64,
    before: Vec<u8>,
    after: Vec<u8>,
}

impl<W: Write + Seek> Out<W> {
    fn new(w: W, format: SectorFormat) -> Self {
        let d = format.data_offset() as usize;
        let tail = format.stored_size() as usize - d - S;
        // The bytes around the data are not interpreted (specification 1, 1.2 item 1).
        Out { w, format, lsn: 0, before: vec![0; d], after: vec![0; tail] }
    }

    fn seek(&mut self, lsn: u64) -> io::Result<()> {
        self.w.seek(SeekFrom::Start(lsn * self.format.stored_size()))?;
        self.lsn = lsn;
        Ok(())
    }

    fn sector(&mut self, data: &[u8]) -> io::Result<()> {
        debug_assert_eq!(data.len(), S);
        self.w.write_all(&self.before)?;
        self.w.write_all(data)?;
        self.w.write_all(&self.after)?;
        self.lsn += 1;
        Ok(())
    }

    fn sectors(&mut self, data: &[u8]) -> io::Result<()> {
        data.chunks_exact(S).try_for_each(|s| self.sector(s))
    }
}

/// The audio sector under construction.
#[derive(Default)]
struct Sector {
    dst: Option<bool>,
    /// Frame records: timecode and (DST) sector count.
    starts: Vec<(u32, u8)>,
    /// Packets: frame start bit and length.
    packets: Vec<(bool, usize)>,
    data: Vec<u8>,
}

/// Packs frames into audio sectors (specification 4, 4.3) and writes them.
struct Packer {
    cur: Sector,
    written: u64,
    buf: Box<[u8; S]>,
}

impl Packer {
    fn new() -> Self {
        Packer { cur: Sector::default(), written: 0, buf: Box::new([0; S]) }
    }

    /// Adds a frame; returns the index (from the area's first audio sector) of the sector
    /// that starts it.
    fn add<W: Write + Seek>(
        &mut self,
        out: &mut Out<W>,
        number: u32,
        dst: bool,
        data: &[u8],
    ) -> io::Result<u64> {
        let mut off = 0;
        let mut first = 0;
        while off < data.len() {
            let starting = off == 0;
            let cur = &self.cur;
            let flag = cur.dst.unwrap_or(dst);
            let r = if flag { 4 } else { 3 };
            let new = cur.packets.len() == 7 || (starting && (cur.starts.len() == 7 || flag != dst));
            let h = 1 + 2 * (cur.packets.len() + 1) + (cur.starts.len() + usize::from(starting)) * r;
            if new || h + cur.data.len() >= S {
                self.flush(out)?;
                continue;
            }
            let n = (data.len() - off).min(S - h - cur.data.len());
            if starting {
                // Later pieces each fill a sector of their own, as the only packet.
                let packets = 1 + (data.len() - n).div_ceil(CONTINUATION_BYTES);
                if dst && packets > MAX_DST_PACKETS {
                    return Err(io::Error::other("DST frame too large for its sector count"));
                }
                self.cur.starts.push((number, packets as u8));
                first = self.written;
            }
            self.cur.dst = Some(flag);
            self.cur.packets.push((starting, n));
            self.cur.data.extend_from_slice(&data[off..off + n]);
            off += n;
        }
        Ok(first)
    }

    fn flush<W: Write + Seek>(&mut self, out: &mut Out<W>) -> io::Result<()> {
        let s = std::mem::take(&mut self.cur);
        if s.packets.is_empty() {
            return Ok(());
        }
        let dst = s.dst.unwrap_or(false);
        let r = if dst { 4 } else { 3 };
        let audio_packets = s.packets.len();
        let mut packets = s.packets;
        // A padding packet fills the rest of the sector when its record fits.
        let h = 1 + 2 * (packets.len() + 1) + s.starts.len() * r;
        if packets.len() < 7 && h + s.data.len() < S {
            packets.push((false, S - h - s.data.len()));
        }
        let b = &mut self.buf;
        b.fill(0);
        b[0] = u8::from(dst) | ((s.starts.len() as u8) << 2) | ((packets.len() as u8) << 5);
        let mut at = 1;
        for (i, &(start, len)) in packets.iter().enumerate() {
            let kind: u16 = if i < audio_packets { 2 } else { 7 };
            put16(&mut b[..], at, (u16::from(start) << 15) | (kind << 11) | len as u16);
            at += 2;
        }
        for &(number, count) in &s.starts {
            put(&mut b[..], at, &tc(number));
            at += 3;
            if dst {
                b[at] = count << 2;
                at += 1;
            }
        }
        put(&mut b[..], at, &s.data);
        out.sector(&b[..])?;
        self.written += 1;
        // Keep the allocations.
        self.cur.data = s.data;
        self.cur.data.clear();
        Ok(())
    }
}

/// A deterministic pseudo-random DSD signal (xorshift64*).
struct Noise(u64);

impl Noise {
    fn fill(&mut self, out: &mut [u8]) {
        for chunk in out.chunks_mut(8) {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            let v = x.wrapping_mul(0x2545_F491_4F6C_DD1D).to_le_bytes();
            chunk.copy_from_slice(&v[..chunk.len()]);
        }
    }
}

fn area_config(channels: u8) -> u8 {
    match channels {
        5 => 3,
        6 => 4,
        _ => 0,
    }
}

/// The area TOC (specification 3) of an area whose layout is known.
fn area_toc(a: &AreaLayout, kind: Kind, frames: u32) -> Vec<u8> {
    let mut z = vec![0u8; AREA_TOC_SECTORS as usize * S];
    let c = a.channels;
    put(&mut z, 0, if a.slot == Slot::TwoChannel { b"TWOCHTOC" } else { b"MULCHTOC" });
    z[8] = 1;
    z[9] = 20;
    put16(&mut z, 10, AREA_TOC_SECTORS as u16);
    put32(&mut z, 16, u32::from(c) * FRAME_BYTES_PER_CHANNEL as u32 * 75);
    z[20] = 4;
    // 0: DST; 3: plain DSD ("3 in 16"; informative, specification 3, 3.4.2).
    z[21] = if kind == Kind::Dst { 0 } else { 3 };
    z[32] = c;
    z[33] = area_config(c);
    z[34] = c;
    put(&mut z, 64, &tc(frames));
    z[69] = a.tracks.len() as u8;
    put32(&mut z, 72, a.audio.0);
    put32(&mut z, 76, a.audio.1);
    z[80] = 1;
    put(&mut z, 88, b"en\x02");
    let ttxt = S;
    let igl = 2 * S;
    let trl1 = 4 * S;
    let trl2 = 5 * S;
    put(&mut z, ttxt, b"SACDTTxt");
    put(&mut z, igl, b"SACD_IGL");
    put(&mut z, trl1, b"SACDTRL1");
    put(&mut z, trl2, b"SACDTRL2");
    // Track text after the 255 offsets, while it fits in the sector.
    let mut text_at = 8 + 2 * 255 + 2;
    for (i, &(lsn, len, start, duration)) in a.tracks.iter().enumerate() {
        put32(&mut z, trl1 + 8 + 4 * i, lsn);
        put32(&mut z, trl1 + 1028 + 4 * i, len);
        put(&mut z, trl2 + 8 + 4 * i, &tc(start));
        put(&mut z, trl2 + 1028 + 4 * i, &tc(duration));
        put(&mut z, igl + 8 + 12 * i, format!("XXSYN26{:05}", i + 1).as_bytes());
        put(&mut z, igl + 3072 + 4 * i, &[1, 0, 0, 5]);
        let title = format!("Track {}\0", i + 1);
        if text_at + 6 + title.len() <= S {
            put16(&mut z, ttxt + 8 + 2 * i, text_at as u16);
            put(&mut z, ttxt + text_at, &[1, 0, 0, 0, 1, 0x20]);
            put(&mut z, ttxt + text_at + 6, title.as_bytes());
            text_at += 6 + title.len();
        }
    }
    z
}

/// The master TOC (specification 2): 10 sectors.
fn master_toc(areas: &[AreaLayout]) -> Vec<u8> {
    let mut m = vec![0u8; 10 * S];
    put(&mut m, 0, b"SACDMTOC");
    m[8] = 1;
    m[9] = 20;
    put16(&mut m, 16, 1);
    put16(&mut m, 18, 1);
    put(&mut m, 24, b"SYNTH-0001");
    for a in areas {
        let (ptr, size) = if a.slot == Slot::TwoChannel { (64, 84) } else { (72, 86) };
        put32(&mut m, ptr, a.toc_lsns.0);
        put32(&mut m, ptr + 4, a.toc_lsns.1);
        put16(&mut m, size, AREA_TOC_SECTORS as u16);
    }
    put(&mut m, 88, b"SYNTH-0001");
    m[128] = 1;
    put(&mut m, 136, b"en\x02");
    let t = S;
    put(&mut m, t, b"SACDText");
    // Album title and disc title share one string.
    put16(&mut m, t + 16, 48);
    put16(&mut m, t + 32, 48);
    put(&mut m, t + 48, b"Synthetic benchmark image\0");
    put(&mut m, 9 * S, b"SACD_Man");
    m
}

/// Checks a configuration.
///
/// # Errors
///
/// A message for a configuration that cannot be written.
pub fn check(cfg: &Config) -> Result<(), String> {
    if !(cfg.two_channel || cfg.multichannel) {
        return Err("no area selected".into());
    }
    if cfg.frames == 0 || cfg.frames > MAX_FRAMES {
        return Err(format!("duration must be 1 to {MAX_FRAMES} frames"));
    }
    if cfg.track_frames == 0 || cfg.frames.div_ceil(cfg.track_frames) > 255 {
        return Err("at most 255 tracks".into());
    }
    if cfg.multichannel && !matches!(cfg.mc_channels, 5 | 6) {
        return Err("multichannel areas have 5 or 6 channels".into());
    }
    Ok(())
}

fn write_area<W: Write + Seek>(
    out: &mut Out<W>,
    cfg: &Config,
    slot: Slot,
    channels: u8,
    toc1: u32,
    src: &DstSource,
) -> io::Result<AreaLayout> {
    let audio_start = toc1 + AREA_TOC_SECTORS;
    out.seek(u64::from(audio_start))?;
    let mut packer = Packer::new();
    let mut noise = Noise(0x9E37_79B9_7F4A_7C15 ^ u64::from(channels));
    let mut pcm = vec![0u8; usize::from(channels) * FRAME_BYTES_PER_CHANNEL];
    let dst = src.frames(channels);
    if cfg.kind == Kind::Dst && dst.is_empty() {
        return Err(io::Error::other(format!("no {channels}-channel DST frames")));
    }
    let mut starts = Vec::new();
    for n in 0..cfg.frames {
        let first = match cfg.kind {
            Kind::Dsd => {
                noise.fill(&mut pcm);
                packer.add(out, n, false, &pcm)?
            }
            Kind::Dst => packer.add(out, n, true, &dst[n as usize % dst.len()])?,
        };
        if n % cfg.track_frames == 0 {
            starts.push((n, first));
        }
    }
    packer.flush(out)?;
    let count = packer.written;
    let too_far = || io::Error::other("image too large for 32-bit LSNs");
    let audio_end = u32::try_from(u64::from(audio_start) + count - 1).map_err(|_| too_far())?;
    let toc2 = audio_end.checked_add(1).ok_or_else(too_far)?;
    let tracks = starts
        .iter()
        .enumerate()
        .map(|(i, &(frame, sector))| {
            let next_sector = starts.get(i + 1).map_or(count, |s| s.1);
            let next_frame = starts.get(i + 1).map_or(cfg.frames, |s| s.0);
            (audio_start + sector as u32, (next_sector - sector) as u32, frame, next_frame - frame)
        })
        .collect();
    let layout =
        AreaLayout { slot, channels, toc_lsns: (toc1, toc2), audio: (audio_start, audio_end), tracks };
    let toc = area_toc(&layout, cfg.kind, cfg.frames);
    out.sectors(&toc)?;
    let end = out.lsn;
    out.seek(u64::from(toc1))?;
    out.sectors(&toc)?;
    out.seek(end)?;
    Ok(layout)
}

/// Writes an image to `w`, sector by sector.
///
/// # Errors
///
/// An invalid configuration, missing DST frames, or an I/O error.
pub fn write_image<W: Write + Seek>(w: W, cfg: &Config, src: &DstSource) -> io::Result<Layout> {
    check(cfg).map_err(io::Error::other)?;
    let mut out = Out::new(w, cfg.format);
    // LSN 0 to 509: file system and reserved sectors, left as zeros.
    out.seek(0)?;
    for _ in 0..u64::from(FIRST_AREA_LSN) {
        out.sector(&[0; S])?;
    }
    let mut areas = Vec::new();
    let mut next = FIRST_AREA_LSN;
    for (want, slot, channels) in
        [(cfg.two_channel, Slot::TwoChannel, 2), (cfg.multichannel, Slot::Multichannel, cfg.mc_channels)]
    {
        if want {
            let a = write_area(&mut out, cfg, slot, channels, next, src)?;
            next = a.toc_lsns.1 + AREA_TOC_SECTORS;
            areas.push(a);
        }
    }
    let end = out.lsn;
    let master = master_toc(&areas);
    out.seek(u64::from(scarletbook::MASTER_TOC_LSN))?;
    for _ in 0..3 {
        out.sectors(&master)?;
    }
    out.seek(end)?;
    out.w.flush()?;
    Ok(Layout { areas, sectors: end })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frame number of a timecode (specification 0, 0.5 item 2).
    fn frame_number(t: [u8; 3]) -> u32 {
        (u32::from(t[0]) * 60 + u32::from(t[1])) * 75 + u32::from(t[2])
    }

    #[test]
    fn timecodes_round_trip_up_to_the_largest_frame_number() {
        for n in
            [0, 1, 74, 75, 4499, 4500, 1_151_999, 1_152_000, 1_152_074, 1_152_075, MAX_FRAMES - 1, MAX_FRAMES]
        {
            assert_eq!(frame_number(tc(n)), n, "frame {n}");
        }
        // Normalised up to 255:59:74; s and f carry the rest beyond it.
        assert_eq!(tc(1_151_999), [255, 59, 74]);
        assert_eq!(tc(1_152_000), [255, 60, 0]);
        assert_eq!(tc(MAX_FRAMES), [255, 255, 255]);
        // Every frame number round-trips.
        assert!((0..=MAX_FRAMES).all(|n| frame_number(tc(n)) == n));
    }
}
