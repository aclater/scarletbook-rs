// SPDX-License-Identifier: MIT OR Apache-2.0

//! Output files of an accepted TOC (issue #131), for the `toc` target: the layouts of a
//! range of a hostile TOC as DSF, DSDIFF with DSD and DSDIFF with DST, with writer's choices,
//! DST lengths ([`DstLengths::from_lengths`]) and a raw ID3 tag ([`Tags::Raw`]) taken from
//! the input's last [`CONTROL`] bytes.

use std::borrow::Cow;
use std::io::{Read, Seek};
use std::ops::Range;

use scarletbook::audio::{FRAME_BYTES_PER_CHANNEL, MAX_FRAME_BYTES};
use scarletbook::output::{
    AlbumArtistFrame, ArrangerFrame, Comment, CommentKind, DateFrame, DsfBitsPerSample, DstIndexOffset,
    DstLengths, Format, Id3Encoding, InfoChunk, Input, InvalidDst, Layout, LocalChunk, MAX_TAG_BYTES, Marker,
    MarkerKind, MarkerText, Markers, OutputOptions, PositionSource, PropertyChunk, RegionKind, SourceTrack,
    TagItem, TagSet, Tags, TextSource,
};
use scarletbook::toc::Area;
use scarletbook::{Disc, Error};

/// Bytes at the end of a `toc` input that choose what is checked (zeros beyond the input,
/// and all zeros choose the play range of the first area with the default options):
///
/// | Bytes | Use |
/// |---|---|
/// | 0 | bit 0: the area; bits 1-2: the range (play range, track, track with pause, `[first, end)`); bits 3-7: the track |
/// | 1-4, 5-8 | `first` and `end` (u32, little-endian), reduced modulo `P + 1` unless byte 24 bit 6 is set |
/// | 9-13 | writer's choices of the DSF and DSDIFF formats |
/// | 14-15 | the ID3 writer's choices |
/// | 16-19 | a value for the choices that take one (u32) |
/// | 20-23 | the raw tag's length (u32): the bytes just before these 32 |
/// | 24 | the DST lengths: bits 0-1 their sizes, bit 2 invalid frames replaced, bit 3 one out of bounds, bit 4 one too many; bit 5 the raw tag's header made valid; bit 6 `first` and `end` as they are; bit 7 reads of audio from the disc |
/// | 25-28 | the seed of the DST lengths (u32) |
/// | 29-31 | the seed of the reads |
pub const CONTROL: usize = 32;

/// The most positions of a DST layout: longer ranges are cut, so that the lengths stay small.
const MAX_DST_POSITIONS: u32 = 4096;

/// The control bytes of an input.
struct Control([u8; CONTROL]);

impl Control {
    fn of(data: &[u8]) -> Control {
        let mut c = [0; CONTROL];
        let tail = &data[data.len().saturating_sub(CONTROL)..];
        c[..tail.len()].copy_from_slice(tail);
        Control(c)
    }

    fn bit(&self, byte: usize, bit: u32) -> bool {
        self.0[byte] & (1 << bit) != 0
    }

    fn u32(&self, at: usize) -> u32 {
        u32::from_le_bytes(self.0[at..at + 4].try_into().unwrap())
    }
}

/// A small deterministic generator.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

/// Checks the output layouts of the range and options that the control bytes of `data`
/// choose, for an accepted disc of `data`.
pub fn check_outputs<R: Read + Seek>(disc: &mut Disc<R>, data: &[u8]) {
    let c = Control::of(data);
    let toc = disc.toc().clone();
    let area = &toc.areas[usize::from(c.0[0] & 1) % toc.areas.len()];
    let p = area.playtime_frames();
    let track = u32::from(c.0[0] >> 3) % area.tracks.len().max(1) as u32 + 1;
    let (first, end) = match (c.0[0] >> 1) & 3 {
        0 => (0, p),
        1 => area.track(track).map_or((0, p), |t| (t.start_frame, t.end_frame())),
        2 => disc.track_range_with_pause(area.slot, track).unwrap_or((0, p)),
        _ if c.bit(24, 6) => (c.u32(1), c.u32(5)),
        _ => (c.u32(1) % p.saturating_add(1).max(1), c.u32(5) % p.saturating_add(1).max(1)),
    };
    let options = options(&c, area);
    let tag = raw_tag(&c, data);
    for format in [Format::Dsf, Format::DsdiffDsd] {
        for options in [options.clone(), options.clone().with_tags(Tags::Raw(tag.clone()))] {
            if let Ok(layout) = disc.output_layout(area.slot, first, end, format, &options) {
                if let Tags::Raw(bytes) = &options.tags {
                    if format == Format::Dsf {
                        assert_raw(bytes);
                    }
                }
                check_layout(disc, &layout, None, &c);
            }
        }
    }
    assert!(
        disc.output_layout(area.slot, first, end, Format::DsdiffDst, &options).is_err(),
        "a DST layout needs its lengths"
    );

    // DST: a range of at most MAX_DST_POSITIONS, and lengths from the control bytes.
    let end = end.min(first.saturating_add(MAX_DST_POSITIONS));
    let invalid = if c.bit(24, 2) { InvalidDst::Replaced } else { InvalidDst::Stored };
    let Some(n) = end.checked_sub(first) else {
        assert!(DstLengths::from_lengths(area, first, end, invalid, Vec::new()).is_err());
        return;
    };
    let plain = 1 + u32::from(area.channel_count) * FRAME_BYTES_PER_CHANNEL as u32;
    let mut rng = Lcg(u64::from(c.u32(25)));
    let max = MAX_FRAME_BYTES as u64;
    let mut lengths: Vec<u32> = (0..n)
        .map(|_| match c.0[24] & 3 {
            0 => plain,
            1 => 1 + rng.below(max) as u32,
            2 => 1 + rng.below(64) as u32,
            _ if rng.below(2) == 0 => plain,
            _ => 1 + rng.below(max) as u32,
        })
        .collect();
    let out_of_bounds = c.bit(24, 3) && n > 0;
    if out_of_bounds {
        let i = rng.below(u64::from(n)) as usize;
        lengths[i] = if rng.below(2) == 0 { 0 } else { MAX_FRAME_BYTES as u32 + 1 };
    }
    if c.bit(24, 4) {
        lengths.push(1);
    }
    let channels_ok = (1..=6).contains(&area.channel_count);
    match DstLengths::from_lengths(area, first, end, invalid, lengths.clone()) {
        Ok(dst) => {
            assert!(channels_ok && !out_of_bounds && !c.bit(24, 4), "lengths accepted unchecked");
            assert_eq!(dst.lengths(), &lengths[..]);
            assert_eq!((dst.slot(), dst.range(), dst.invalid()), (area.slot, (first, end), invalid));
            if let Ok(layout) = disc.output_layout_dst(&dst, &options) {
                check_layout(disc, &layout, Some(&lengths), &c);
            }
        }
        Err(e) => {
            assert!(!channels_ok || out_of_bounds || c.bit(24, 4), "lengths refused: {e}");
            assert!(matches!(e, Error::InvalidArgument(_) | Error::ChannelCountUnsupported { .. }), "{e}");
        }
    }
}

/// The writer's choices of the control bytes, valid or not.
fn options(c: &Control, area: &Area) -> OutputOptions {
    let v = c.u32(16);
    let mut o = OutputOptions::default();
    let b = c.0[9];
    o.dsf_bits_per_sample = if b & 1 != 0 { DsfBitsPerSample::Eight } else { DsfBitsPerSample::One };
    o.dsf_unspecified_channel_type = (b & 2 != 0).then_some(v % 9);
    if b & 4 != 0 {
        let count = if b & 8 != 0 { usize::from(area.channel_count) } else { usize::from(c.0[10] % 8) };
        o.dsdiff_unspecified_channel_numbers =
            Some((0..count).map(|i| ((v >> (i % 3)) % 1002) as u16).collect());
    }
    o.dsdiff_unspecified_loudspeaker_configuration = b & 16 != 0;
    o.dsdiff_loudspeaker_configuration = b & 32 == 0;
    o.dsdiff_absolute_start = (b & 64 != 0).then_some(v);
    o.dsdiff_dst_crc = b & 128 != 0;

    let b = c.0[10];
    o.dsdiff_dst_index = b & 1 == 0;
    o.dsdiff_dst_index_offset =
        if b & 2 != 0 { DstIndexOffset::FrameData } else { DstIndexOffset::ChunkHeader };
    o.source_track = match (b >> 2) & 3 {
        0 => SourceTrack::Auto,
        1 => SourceTrack::None,
        _ => SourceTrack::Track((v >> 8) as u8),
    };
    o.dsdiff_edited_master = b & 16 != 0;
    o.dsdiff_edited_master_info = b & 32 != 0;
    if b & 64 != 0 {
        o.dsdiff_edited_master_id = String::from_utf8_lossy(&c.0[16..20]).into_owned();
    }
    if b & 128 != 0 {
        let kinds =
            [MarkerKind::TrackStart, MarkerKind::TrackStop, MarkerKind::ProgramStart, MarkerKind::Index];
        let markers = (0..usize::from(c.0[11] % 4))
            .map(|i| {
                let mut m = Marker::new(
                    v.rotate_left(8 * i as u32) % area.playtime_frames().saturating_add(2),
                    kinds[i],
                );
                m.track_flags = u16::from(c.0[12]);
                m.text = c.0[16..16 + i].to_vec();
                m
            })
            .collect();
        o.dsdiff_markers = Markers::Custom(markers);
    }

    let b = c.0[11];
    o.dsdiff_source_track_markers = b & 4 != 0;
    o.dsdiff_clipped_program = b & 8 != 0;
    o.dsdiff_text_pad_counted = b & 16 != 0;
    o.text_source = [TextSource::DiscThenAlbum, TextSource::Disc, TextSource::Album][usize::from(b >> 5) % 3];
    o.dsdiff_artist_title = [None, Some(true), Some(false), None][usize::from(b >> 6)];

    let b = c.0[12];
    o.tags = match b & 3 {
        0 => Tags::FromToc,
        1 => Tags::None,
        2 => {
            let mut set = TagSet::new();
            set.set(TagItem::TrackTitle, &String::from_utf8_lossy(&c.0[16..20]));
            set.set_custom(&String::from_utf8_lossy(&c.0[17..20]), "x");
            Tags::Set(set)
        }
        _ => Tags::Set(TagSet::new()),
    };
    o.genres.track = b & 4 == 0;
    o.genres.album = b & 8 == 0;
    o.genres.disc = b & 16 == 0;
    o.dsdiff_marker_text = if b & 32 != 0 { MarkerText::TrackTitle } else { MarkerText::Empty };
    if b & 64 != 0 {
        let kind = match v % 5 {
            0 => CommentKind::General,
            1 => CommentKind::Channel((v >> 4) as u16 % 8),
            2 => CommentKind::SoundSource((v >> 4) as u16 % 4),
            3 => CommentKind::FileHistory((v >> 4) as u16 % 6),
            _ => CommentKind::Channel((v >> 4) as u16),
        };
        let mut comment = Comment::new(kind, &String::from_utf8_lossy(&c.0[16..20]));
        comment.month = c.0[13] % 14;
        comment.hour = c.0[13] >> 3;
        o.dsdiff_comments = vec![comment.clone(), comment];
    }
    o.dsdiff_empty_text = b & 128 != 0;

    let b = c.0[13];
    o.dsdiff_text_replacement = match b & 3 {
        0 => Some(b'?'),
        1 => None,
        _ => Some(c.0[16]),
    };
    if b & 4 != 0 {
        let mut order = PropertyChunk::TABLE_ORDER;
        order.rotate_left(usize::from(b >> 5) % 5);
        if b & 8 != 0 {
            order[0] = order[1];
        }
        o.dsdiff_property_order = order;
    }
    if b & 16 != 0 {
        let mut order = LocalChunk::DEFAULT_ORDER.to_vec();
        let len = order.len();
        order.rotate_left(usize::from(b >> 5) % len);
        order.truncate(len - usize::from(c.0[14] >> 6) % 2);
        o.dsdiff_local_order = order;
    }
    if b & 128 != 0 {
        let mut order = InfoChunk::TABLE_ORDER;
        order.swap(0, usize::from(b >> 5) % 4);
        o.dsdiff_info_order = order;
    }

    let b = c.0[14];
    let id3 = &mut o.id3;
    id3.footer = b & 1 == 0;
    if b & 2 != 0 {
        id3.padding = v % (2 * MAX_TAG_BYTES as u32);
    }
    id3.encoding = [
        Id3Encoding::Utf8,
        Id3Encoding::Latin1OrUtf8,
        Id3Encoding::Utf16Le,
        Id3Encoding::Utf16Be,
        Id3Encoding::Utf16BeNoBom,
    ][usize::from(b >> 2) % 5];
    id3.terminate_last = b & 32 != 0;
    id3.control_replacement = (b & 64 != 0).then(|| char::from(c.0[17]));
    if b & 128 != 0 {
        id3.split = Some(String::from_utf8_lossy(&c.0[18..19 + usize::from(c.0[15] & 1)]).into_owned());
    }
    let b = c.0[15];
    id3.arranger = if b & 2 != 0 { ArrangerFrame::Txxx } else { ArrangerFrame::Tipl };
    if b & 4 != 0 {
        id3.join_messages = Some(String::from_utf8_lossy(&c.0[19..20]).into_owned());
    }
    id3.phonetic = b & 8 != 0;
    id3.track_count = b & 16 == 0;
    id3.album_artist =
        [AlbumArtistFrame::None, AlbumArtistFrame::Tpe2, AlbumArtistFrame::Txxx, AlbumArtistFrame::None]
            [usize::from(b >> 5) % 4];
    id3.copyright = b & 32 == 0;
    id3.album_set_alone = b & 64 != 0;
    id3.date = if b & 128 != 0 { DateFrame::Tdrc } else { DateFrame::Tdrl };
    if b & 2 != 0 && b & 1 != 0 {
        id3.language = [c.0[16], c.0[17], c.0[18]];
        id3.frame_order.reverse();
        id3.frame_order.truncate(usize::from(c.0[19]) % 20);
    }
    o
}

/// The raw tag of the control bytes: the input's bytes just before them, with its header
/// made valid when byte 24 bit 5 says so.
fn raw_tag(c: &Control, data: &[u8]) -> Vec<u8> {
    let before = &data[..data.len().saturating_sub(CONTROL)];
    let len = (c.u32(20) as usize) % (before.len() + 1);
    let mut tag = before[before.len() - len..].to_vec();
    if c.bit(24, 5) && tag.len() >= 10 {
        let footer = if tag[5] & 0x10 != 0 { 10 } else { 0 };
        if let Some(size) = tag.len().checked_sub(10 + footer) {
            tag[..5].copy_from_slice(b"ID3\x04\x00");
            for (i, b) in tag[6..10].iter_mut().enumerate() {
                *b = (size >> (7 * (3 - i)) & 0x7F) as u8;
            }
        }
    }
    tag
}

/// A raw tag accepted for a DSF file: an ID3v2.4 header with its stated size.
fn assert_raw(tag: &[u8]) {
    assert!(tag.len() >= 10 && tag.len() <= MAX_TAG_BYTES && tag.starts_with(b"ID3\x04\x00"));
    assert!(tag[6..10].iter().all(|b| b & 0x80 == 0));
    let size = tag[6..10].iter().fold(0, |v, &b| v << 7 | usize::from(b));
    assert_eq!(10 + size + if tag[5] & 0x10 != 0 { 10 } else { 0 }, tag.len());
}

/// Synthetic positions: content and DST equivalents of the given lengths, slices of one
/// pattern at an offset of the position's number, and what was asked of it.
struct Synthetic<'a> {
    first: u32,
    channels: u8,
    lengths: Option<&'a [u32]>,
    pattern: Vec<u8>,
    asked: Vec<(u32, bool)>,
}

impl<'a> Synthetic<'a> {
    fn new(first: u32, channels: u8, lengths: Option<&'a [u32]>) -> Self {
        let pattern = (0..MAX_FRAME_BYTES + 256)
            .map(|i| (i as u32).wrapping_mul(0x9E37_79B9).to_be_bytes()[0])
            .collect();
        Synthetic { first, channels, lengths, pattern, asked: Vec::new() }
    }

    fn make(&self, t: u32, len: usize) -> Cow<'_, [u8]> {
        let at = (t % 251) as usize;
        Cow::Borrowed(&self.pattern[at..at + len])
    }
}

impl PositionSource for Synthetic<'_> {
    fn content(&mut self, t: u32) -> Result<Cow<'_, [u8]>, Error> {
        self.asked.push((t, false));
        let len = usize::from(self.channels) * FRAME_BYTES_PER_CHANNEL;
        Ok(self.make(t, len))
    }

    fn dst_equivalent(&mut self, t: u32) -> Result<Cow<'_, [u8]>, Error> {
        self.asked.push((t, true));
        let lengths = self.lengths.ok_or(Error::InvalidArgument("no DST equivalents"))?;
        let len = lengths[(t - self.first) as usize] as usize;
        Ok(self.make(t, len))
    }
}

/// Checks a layout: its regions tile `[0, size)` and are bounded; reads at region
/// boundaries, at the end, past it and at offsets of the control bytes fill the bytes
/// [`Layout::fill`] promises, read only positions of [`Layout::positions_for`], give the
/// same bytes when split in two, and agree with [`Layout::read_at`] outside the sound data.
fn check_layout<R: Read + Seek>(disc: &mut Disc<R>, layout: &Layout, lengths: Option<&[u32]>, c: &Control) {
    let (first, end) = layout.range();
    let n = u64::from(end - first);
    let seed = u64::from(u32::from_le_bytes([c.0[29], c.0[30], c.0[31], 0]));
    let c_decode = c.bit(24, 7);
    let c = u64::from(layout.channels());
    let size = layout.size();
    let regions = layout.regions();
    assert!(!regions.is_empty() && regions.len() <= 16, "{} regions", regions.len());
    assert_eq!(regions[0].kind, RegionKind::Header);
    let mut at = 0;
    for r in regions {
        assert_eq!(r.offset, at);
        at = at.checked_add(r.len).unwrap();
    }
    assert_eq!(at, size);
    let sound: Vec<_> = regions.iter().filter(|r| r.kind == RegionKind::SoundData).collect();
    assert_eq!(sound.len(), 1);
    let audio = n * c * FRAME_BYTES_PER_CHANNEL as u64;
    match lengths {
        None => {
            assert_eq!(layout.input(), Input::Content);
            assert!(
                sound[0].len >= audio && sound[0].len <= audio + c * 4096 + 64,
                "{} for {audio}",
                sound[0].len
            );
        }
        Some(lengths) => {
            assert_ne!(layout.input(), Input::Content);
            let units: u64 = lengths.iter().map(|&l| 12 + u64::from(l).next_multiple_of(2)).sum();
            assert!(
                sound[0].len >= units && sound[0].len <= units + 16 * n + 64,
                "{} for {units}",
                sound[0].len
            );
        }
    }
    let bytes = size - sound[0].len;
    let indexed = if lengths.is_some() { 12 * n } else { 0 };
    // The tag, and the TOC's text (at most 128 area TOC sectors) in markers and comments.
    assert!(bytes <= (1 << 20) + MAX_TAG_BYTES as u64 + indexed, "{bytes} bytes besides the sound data");
    assert!(layout.memory() <= 1024 + 2 * bytes as usize + 16 * n as usize, "{} bytes held", layout.memory());
    let whole = layout.positions_for(0..size);
    assert!(whole.start >= first && whole.end <= end, "{whole:?} of [{first}, {end})");
    assert!(layout.positions_for(0..regions[0].len).is_empty());

    // Short reads at the region boundaries, the end and past it, and two longer ones (up to
    // three positions, a few DSF blocks) at offsets of the control bytes.
    let mut reads: Vec<(u64, u64)> = vec![(0, 64), (size.saturating_sub(1), 2), (size, 4), (u64::MAX - 2, 8)];
    for r in &regions[1..] {
        reads.push((r.offset - 1, 2));
        reads.push((r.offset.saturating_sub(32), 64));
    }
    let span = 3 * c * FRAME_BYTES_PER_CHANNEL as u64;
    let mut rng = Lcg(seed ^ size);
    for _ in 0..2 {
        let at = rng.below(size);
        reads.push((at, 1 + rng.below(span)));
    }
    let mut source = Synthetic::new(first, layout.channels(), lengths);
    let decode = c_decode;
    for (at, len) in reads {
        check_read(disc, layout, &mut source, at, len as usize, &mut rng, decode);
    }
}

/// One read of [`check_layout`].
fn check_read<R: Read + Seek>(
    disc: &mut Disc<R>,
    layout: &Layout,
    source: &mut Synthetic<'_>,
    at: u64,
    len: usize,
    rng: &mut Lcg,
    decode: bool,
) {
    let size = layout.size();
    let want = usize::try_from(size.saturating_sub(at)).map_or(len, |rest| rest.min(len));
    let (first, end) = layout.range();
    let positions: Range<u32> = layout.positions_for(at..at.saturating_add(len as u64));
    assert!(positions.is_empty() || (positions.start >= first && positions.end <= end), "{positions:?}");

    source.asked.clear();
    let mut whole = vec![0xA5; len + 1];
    assert_eq!(layout.fill(at, &mut whole[..len], source).unwrap(), want);
    assert_eq!(whole[len], 0xA5, "fill wrote past its buffer");
    for &(t, dst) in &source.asked {
        assert!(positions.contains(&t), "fill of {at}+{len} read {t}, outside {positions:?}");
        if layout.input() == Input::Content {
            assert!(!dst, "a DST equivalent of a DSD layout");
        } else if layout.input() == Input::DstEquivalents {
            assert!(dst, "content of a DST layout without CRCs");
        }
    }

    // The same bytes in two reads.
    if want > 0 {
        let k = rng.below(want as u64) as usize;
        let mut split = vec![0x5A; want];
        assert_eq!(layout.fill(at, &mut split[..k], source).unwrap(), k);
        assert_eq!(layout.fill(at + k as u64, &mut split[k..], source).unwrap(), want - k);
        assert!(split == whole[..want], "fill of {at}+{want} split at {k} differs");
    }

    // From the disc: the bytes besides the sound data are the layout's own. Reading
    // positions decodes them, which is slow: only when byte 24 bit 7 says so.
    if !positions.is_empty() && !decode {
        return;
    }
    let mut got = vec![0; len];
    if let Ok(m) = layout.read_at(disc, at, &mut got) {
        assert_eq!(m, want);
        for r in layout.regions().iter().filter(|r| r.kind != RegionKind::SoundData) {
            let lo = r.offset.max(at);
            let hi = (r.offset + r.len).min(at + want as u64);
            if lo < hi {
                let (lo, hi) = ((lo - at) as usize, (hi - at) as usize);
                assert!(got[lo..hi] == whole[lo..hi], "read_at and fill differ outside the sound data");
            }
        }
    }
}
