// SPDX-License-Identifier: MIT OR Apache-2.0

//! The master TOC (specification 2) and the area TOCs (specification 3).

use std::io::{Read, Seek};

use crate::error::{Error, Slot, Warning, WarningCode};
use crate::image::{Image, MASTER_TOC_LSN, ReadOutcome, SECTOR_SIZE, SectorFormat};
use crate::text::{self, Charset};

/// Largest area TOC in sectors (specification 3, 3.2).
pub const MAX_AREA_TOC_SECTORS: u16 = 128;

/// The complete table of contents of a disc image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toc {
    /// The image's stored sector format.
    pub sector_format: SectorFormat,
    /// The number of logical sectors in the image.
    pub sectors: u64,
    /// The master TOC.
    pub master: MasterToc,
    /// The areas, in slot order (specification 3, 3.1).
    pub areas: Vec<Area>,
}

impl Toc {
    /// The area that came from `slot`, if any.
    pub fn area(&self, slot: Slot) -> Option<&Area> {
        self.areas.iter().find(|a| a.slot == slot)
    }
}

/// A structure version number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Version {
    /// Major number.
    pub major: u8,
    /// Minor number.
    pub minor: u8,
}

/// A genre entry (specification 2, 2.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Genre {
    /// The genre table (category) byte.
    pub category: u8,
    /// The genre index byte.
    pub index: u8,
}

impl Genre {
    fn parse(b: &[u8]) -> Genre {
        Genre { category: byte(b, 0), index: byte(b, 3) }
    }

    /// The informative name of a genre of the general table (1), if it has one.
    pub fn name(self) -> Option<&'static str> {
        if self.category != 1 {
            return None;
        }
        GENERAL_GENRES.get(usize::from(self.index)).copied()
    }
}

const GENERAL_GENRES: [&str; 30] = [
    "not used",
    "not defined",
    "Adult Contemporary",
    "Alternative Rock",
    "Children's Music",
    "Classical",
    "Contemporary Christian",
    "Country",
    "Dance",
    "Easy Listening",
    "Erotic",
    "Folk",
    "Gospel",
    "Hip Hop",
    "Jazz",
    "Latin",
    "Musical",
    "New Age",
    "Opera",
    "Operetta",
    "Pop Music",
    "Rap",
    "Reggae",
    "Rock Music",
    "Rhythm & Blues",
    "Sound Effects",
    "Sound Track",
    "Spoken Word",
    "World Music",
    "Blues",
];

/// A date as stored (specification 2, 2.3.2); 0 means "not set".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Date {
    /// Year.
    pub year: u16,
    /// Month.
    pub month: u8,
    /// Day.
    pub day: u8,
}

/// A locale entry (specification 2, 2.5.1).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Locale {
    /// The language code, trailing NUL and space removed; may contain control characters.
    pub language: String,
    /// The stored character set code byte.
    pub charset: u8,
}

impl Locale {
    fn parse(b: &[u8]) -> Locale {
        Locale { language: text::language([byte(b, 0), byte(b, 1)]), charset: byte(b, 2) }
    }
}

/// A timecode (specification 0, 0.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Timecode {
    /// Minutes.
    pub minutes: u8,
    /// Seconds (not validated).
    pub seconds: u8,
    /// Frames (not validated).
    pub frames: u8,
}

impl Timecode {
    /// The timecode's frame number, `(m x 60 + s) x 75 + f`; at most 1 166 880.
    pub fn frame_number(self) -> u32 {
        (u32::from(self.minutes) * 60 + u32::from(self.seconds)) * 75 + u32::from(self.frames)
    }

    pub(crate) fn parse(b: &[u8]) -> Timecode {
        Timecode { minutes: byte(b, 0), seconds: byte(b, 1), frames: byte(b, 2) }
    }
}

/// Where the master TOC says an area's TOC copies are (specification 3, 3.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AreaPointer {
    /// TOC-1 LSN; 0 means the slot is unused.
    pub toc1_lsn: u32,
    /// TOC-2 LSN; 0 means no backup copy.
    pub toc2_lsn: u32,
    /// Area TOC size in sectors; 0 means the slot is unused.
    pub toc_sectors: u16,
}

impl AreaPointer {
    /// Whether the slot is used.
    pub fn is_used(self) -> bool {
        self.toc1_lsn != 0 && self.toc_sectors != 0
    }
}

macro_rules! text_struct {
    (
        $(#[$meta:meta])* $name:ident, $item:ident { $($(#[$fmeta:meta])* $field:ident = $key:literal => $variant:ident),* $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
        pub struct $name {
            $($(#[$fmeta])* pub $field: Option<String>,)*
        }

        /// Identifies one text item.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $item {
            $($(#[$fmeta])* $variant,)*
        }

        impl $item {
            /// Every item, in table order.
            pub const ALL: &'static [$item] = &[$($item::$variant),*];

            /// The item's name in the conformance manifest.
            pub fn name(self) -> &'static str {
                match self { $($item::$variant => $key,)* }
            }
        }

        impl $name {
            /// The text of one item, if present.
            pub fn get(&self, item: $item) -> Option<&str> {
                match item { $($item::$variant => self.$field.as_deref(),)* }
            }

            fn slot(&mut self, item: $item) -> &mut Option<String> {
                match item { $($item::$variant => &mut self.$field,)* }
            }

            /// The present items and their text, in table order.
            pub fn iter(&self) -> impl Iterator<Item = ($item, &str)> + '_ {
                $item::ALL.iter().filter_map(move |&i| self.get(i).map(|t| (i, t)))
            }

            /// Whether no item is present.
            pub fn is_empty(&self) -> bool {
                self.iter().next().is_none()
            }
        }
    };
}

text_struct! {
    /// Disc text: the 16 items of the first text channel (specification 2, 2.5.4).
    DiscText, DiscTextItem {
        /// Album title.
        album_title = "album_title" => AlbumTitle,
        /// Album artist.
        album_artist = "album_artist" => AlbumArtist,
        /// Album publisher.
        album_publisher = "album_publisher" => AlbumPublisher,
        /// Album copyright.
        album_copyright = "album_copyright" => AlbumCopyright,
        /// Album title, phonetic.
        album_title_phonetic = "album_title_phonetic" => AlbumTitlePhonetic,
        /// Album artist, phonetic.
        album_artist_phonetic = "album_artist_phonetic" => AlbumArtistPhonetic,
        /// Album publisher, phonetic.
        album_publisher_phonetic = "album_publisher_phonetic" => AlbumPublisherPhonetic,
        /// Album copyright, phonetic.
        album_copyright_phonetic = "album_copyright_phonetic" => AlbumCopyrightPhonetic,
        /// Disc title.
        disc_title = "disc_title" => DiscTitle,
        /// Disc artist.
        disc_artist = "disc_artist" => DiscArtist,
        /// Disc publisher.
        disc_publisher = "disc_publisher" => DiscPublisher,
        /// Disc copyright.
        disc_copyright = "disc_copyright" => DiscCopyright,
        /// Disc title, phonetic.
        disc_title_phonetic = "disc_title_phonetic" => DiscTitlePhonetic,
        /// Disc artist, phonetic.
        disc_artist_phonetic = "disc_artist_phonetic" => DiscArtistPhonetic,
        /// Disc publisher, phonetic.
        disc_publisher_phonetic = "disc_publisher_phonetic" => DiscPublisherPhonetic,
        /// Disc copyright, phonetic.
        disc_copyright_phonetic = "disc_copyright_phonetic" => DiscCopyrightPhonetic,
    }
}

text_struct! {
    /// Area text (specification 3, 3.6).
    AreaText, AreaTextItem {
        /// Area description.
        description = "description" => Description,
        /// Area copyright.
        copyright = "copyright" => Copyright,
        /// Area description, phonetic.
        description_phonetic = "description_phonetic" => DescriptionPhonetic,
        /// Area copyright, phonetic.
        copyright_phonetic = "copyright_phonetic" => CopyrightPhonetic,
    }
}

text_struct! {
    /// Track text (specification 3, 3.8).
    TrackText, TrackTextItem {
        /// Title (type 0x01).
        title = "title" => Title,
        /// Performer (type 0x02).
        performer = "performer" => Performer,
        /// Songwriter (type 0x03).
        songwriter = "songwriter" => Songwriter,
        /// Composer (type 0x04).
        composer = "composer" => Composer,
        /// Arranger (type 0x05).
        arranger = "arranger" => Arranger,
        /// Message (type 0x06).
        message = "message" => Message,
        /// Extra message (type 0x07).
        extra_message = "extra_message" => ExtraMessage,
        /// Title, phonetic (type 0x81).
        title_phonetic = "title_phonetic" => TitlePhonetic,
        /// Performer, phonetic (type 0x82).
        performer_phonetic = "performer_phonetic" => PerformerPhonetic,
        /// Songwriter, phonetic (type 0x83).
        songwriter_phonetic = "songwriter_phonetic" => SongwriterPhonetic,
        /// Composer, phonetic (type 0x84).
        composer_phonetic = "composer_phonetic" => ComposerPhonetic,
        /// Arranger, phonetic (type 0x85).
        arranger_phonetic = "arranger_phonetic" => ArrangerPhonetic,
        /// Message, phonetic (type 0x86).
        message_phonetic = "message_phonetic" => MessagePhonetic,
        /// Extra message, phonetic (type 0x87).
        extra_message_phonetic = "extra_message_phonetic" => ExtraMessagePhonetic,
    }
}

impl TrackTextItem {
    /// The item for a stored item type byte, if the type is defined.
    pub fn from_type(t: u8) -> Option<TrackTextItem> {
        let base = t & 0x7F;
        if !(1..=7).contains(&base) {
            return None;
        }
        let idx = usize::from(base - 1) + if t & 0x80 != 0 { 7 } else { 0 };
        TrackTextItem::ALL.get(idx).copied()
    }
}

/// The master TOC (specification 2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MasterToc {
    /// Version (offsets 8 and 9).
    pub version: Version,
    /// Album set size (offset 16).
    pub album_set_size: u16,
    /// Album sequence number (offset 18).
    pub album_sequence_number: u16,
    /// Album catalogue number (offset 24), a fixed text field.
    pub album_catalog_number: String,
    /// Album genres (offset 40).
    pub album_genres: [Genre; 4],
    /// The 2-channel slot's pointers.
    pub two_channel: AreaPointer,
    /// The multichannel slot's pointers.
    pub multichannel: AreaPointer,
    /// Disc flags byte (offset 80).
    pub disc_flags: u8,
    /// Disc catalogue number (offset 88), a fixed text field.
    pub disc_catalog_number: String,
    /// Disc genres (offset 104).
    pub disc_genres: [Genre; 4],
    /// Date (offsets 120 to 123).
    pub date: Date,
    /// Stored text channel count (offset 128).
    pub text_channel_count: u8,
    /// The reported locale list: the first `min(count, 8)` entries.
    pub locales: Vec<Locale>,
    /// Character set code of locale entry 0 (offset 138), used for disc text.
    pub text_charset: u8,
    /// Disc text of the first text channel.
    pub text: DiscText,
    /// Bytes 8 to 2047 of the manufacturer sector, if its signature is present.
    pub manufacturer: Option<Vec<u8>>,
}

impl MasterToc {
    /// Bit 7 of the disc flags: the disc also has a CD layer.
    pub fn hybrid(&self) -> bool {
        self.disc_flags & 0x80 != 0
    }

    /// The pointers of a slot.
    pub fn pointer(&self, slot: Slot) -> AreaPointer {
        match slot {
            Slot::TwoChannel => self.two_channel,
            Slot::Multichannel => self.multichannel,
        }
    }
}

/// An area's kind, from its TOC signature (specification 3, 3.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AreaKind {
    /// `TWOCHTOC`.
    TwoChannel,
    /// `MULCHTOC`.
    Multichannel,
}

impl AreaKind {
    /// The signature string.
    pub fn signature(self) -> &'static str {
        match self {
            AreaKind::TwoChannel => "TWOCHTOC",
            AreaKind::Multichannel => "MULCHTOC",
        }
    }
}

/// An audio area (specification 3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Area {
    /// The master TOC slot it came from.
    pub slot: Slot,
    /// Its kind, from the signature.
    pub kind: AreaKind,
    /// Version (offsets 8 and 9).
    pub version: Version,
    /// LSN of the TOC copy used.
    pub toc_lsn: u32,
    /// Area TOC size in sectors, from the master TOC.
    pub toc_sectors: u16,
    /// Maximum byte rate (offset 16).
    pub max_byte_rate: u32,
    /// Sample frequency code (offset 20); 4 is DSD64.
    pub sample_frequency_code: u8,
    /// Frame format, the low 4 bits of offset 21 (informative only).
    pub frame_format: u8,
    /// Channel count (offset 32).
    pub channel_count: u8,
    /// Area configuration byte (offset 33); the low 3 bits are the loudspeaker configuration.
    pub area_config: u8,
    /// Maximum available channels (offset 34).
    pub max_available_channels: u8,
    /// Area mute flags (offset 35).
    pub area_mute_flags: u8,
    /// Track attribute, the low 4 bits of offset 48.
    pub track_attribute: u8,
    /// Total area play time (offset 64).
    pub playtime: Timecode,
    /// Track offset (offset 68).
    pub track_offset: u8,
    /// First audio LSN (offset 72).
    pub audio_start_lsn: u32,
    /// Last audio LSN, inclusive (offset 76).
    pub audio_end_lsn: u32,
    /// Stored text channel count (offset 80).
    pub text_channel_count: u8,
    /// The reported locale list: the first `min(count, 10)` entries.
    pub locales: Vec<Locale>,
    /// Character set code of area locale entry 0 (offset 90), used for all area text.
    pub text_charset: u8,
    /// Area text.
    pub text: AreaText,
    /// The tracks, numbered 1 to the track count.
    pub tracks: Vec<Track>,
}

impl Area {
    /// The play time as a frame number.
    pub fn playtime_frames(&self) -> u32 {
        self.playtime.frame_number()
    }

    /// The track with the given number (1-based).
    pub fn track(&self, number: u32) -> Option<&Track> {
        number.checked_sub(1).and_then(|i| self.tracks.get(i as usize))
    }
}

/// A track (specification 3, 3.9.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    /// Track number, 1 to the track count.
    pub number: u8,
    /// Start LSN, as stored.
    pub start_lsn: u32,
    /// Length in sectors, as stored.
    pub length_sectors: u32,
    /// Start frame number, after clamping (specification 3, 3.9.2).
    pub start_frame: u32,
    /// Duration in frames, after clamping.
    pub duration_frames: u32,
    /// Start flags byte.
    pub start_flags: u8,
    /// Duration flags byte.
    pub duration_flags: u8,
    /// ISRC, if any.
    pub isrc: Option<String>,
    /// Genre, if the area has a `SACD_IGL`.
    pub genre: Option<Genre>,
    /// Track text.
    pub text: TrackText,
}

impl Track {
    /// The end frame number, `start_frame + duration_frames` (at most the play time).
    pub fn end_frame(&self) -> u32 {
        self.start_frame.saturating_add(self.duration_frames)
    }
}

fn byte(b: &[u8], at: usize) -> u8 {
    b.get(at).copied().unwrap_or(0)
}

fn be16(b: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([byte(b, at), byte(b, at.saturating_add(1))])
}

fn be32(b: &[u8], at: usize) -> u32 {
    let a = |i: usize| byte(b, at.saturating_add(i));
    u32::from_be_bytes([a(0), a(1), a(2), a(3)])
}

fn sub(b: &[u8], at: usize, len: usize) -> &[u8] {
    b.get(at..at.saturating_add(len)).unwrap_or(&[])
}

fn genres(b: &[u8], at: usize) -> [Genre; 4] {
    std::array::from_fn(|i| Genre::parse(sub(b, at + 4 * i, 4)))
}

/// Reads the whole TOC (specifications 2 and 3), returning it with its warnings.
pub(crate) fn read_toc<R: Read + Seek>(img: &mut Image<R>) -> Result<(Toc, Vec<Warning>), Error> {
    let mut warnings = Vec::new();
    let master = read_master(img, &mut warnings)?;
    let mut areas = Vec::new();
    for slot in Slot::ALL {
        let ptr = master.pointer(slot);
        if !ptr.is_used() {
            continue;
        }
        if let Some(area) = read_area(img, slot, ptr, &mut warnings)? {
            areas.push(area);
        }
    }
    if areas.is_empty() {
        return Err(Error::NoAudioAreas);
    }
    Ok((Toc { sector_format: img.format(), sectors: img.sector_count(), master, areas }, warnings))
}

fn read_master<R: Read + Seek>(img: &mut Image<R>, warnings: &mut Vec<Warning>) -> Result<MasterToc, Error> {
    let mut buf = vec![0u8; 10 * SECTOR_SIZE];
    if img.read_into(u64::from(MASTER_TOC_LSN), &mut buf)? == ReadOutcome::OutOfRange {
        return Err(Error::MasterTocTruncated);
    }
    // TODO(aclater/sacd-spec#4): the optional `master-toc-copies-differ` warning (spec
    // 1.5.1, 7.7.3) is not implemented: the specification does not say whether the copies
    // at LSN 520 and 530 are compared over all 10 sectors or only over the reported values.
    let s0 = sub(&buf, 0, SECTOR_SIZE);
    if sub(s0, 0, 8) != b"SACDMTOC" {
        return Err(Error::NotScarletbook);
    }
    let version = Version { major: byte(s0, 8), minor: byte(s0, 9) };
    if version.major > 1 {
        return Err(Error::MasterTocVersionUnsupported { major: version.major, minor: version.minor });
    }
    let text_channel_count = byte(s0, 128);
    let locales =
        (0..usize::from(text_channel_count.min(8))).map(|i| Locale::parse(sub(s0, 136 + 4 * i, 4))).collect();
    let text_charset = byte(s0, 138);

    let s1 = sub(&buf, SECTOR_SIZE, SECTOR_SIZE);
    let mut disc_text = DiscText::default();
    if sub(s1, 0, 8) == b"SACDText" {
        let cs = Charset::from_code(text_charset);
        for (i, &item) in DiscTextItem::ALL.iter().enumerate() {
            let off = usize::from(be16(s1, 16 + 2 * i));
            if off == 0 || off >= SECTOR_SIZE {
                continue;
            }
            *disc_text.slot(item) = text::text_string(s1, off, cs);
        }
    } else if text_channel_count > 0 {
        warnings.push(Warning::disc(WarningCode::MasterTextMissing));
    }

    let s9 = sub(&buf, 9 * SECTOR_SIZE, SECTOR_SIZE);
    let manufacturer = if sub(s9, 0, 8) == b"SACD_Man" {
        Some(sub(s9, 8, SECTOR_SIZE - 8).to_vec())
    } else {
        warnings.push(Warning::disc(WarningCode::ManufacturerMissing));
        None
    };

    let pointer = |lsn1: usize, lsn2: usize, size: usize| AreaPointer {
        toc1_lsn: be32(s0, lsn1),
        toc2_lsn: be32(s0, lsn2),
        toc_sectors: be16(s0, size),
    };
    Ok(MasterToc {
        version,
        album_set_size: be16(s0, 16),
        album_sequence_number: be16(s0, 18),
        album_catalog_number: text::fixed_text(sub(s0, 24, 16)),
        album_genres: genres(s0, 40),
        two_channel: pointer(64, 68, 84),
        multichannel: pointer(72, 76, 86),
        disc_flags: byte(s0, 80),
        disc_catalog_number: text::fixed_text(sub(s0, 88, 16)),
        disc_genres: genres(s0, 104),
        date: Date { year: be16(s0, 120), month: byte(s0, 122), day: byte(s0, 123) },
        text_channel_count,
        locales,
        text_charset,
        text: disc_text,
        manufacturer,
    })
}

/// Reads one area TOC copy; `None` if any sector is missing.
fn read_copy<R: Read + Seek>(img: &mut Image<R>, lsn: u32, sectors: u16) -> Result<Option<Vec<u8>>, Error> {
    let mut buf = vec![0u8; usize::from(sectors) * SECTOR_SIZE];
    Ok(match img.read_into(u64::from(lsn), &mut buf)? {
        ReadOutcome::Ok => Some(buf),
        ReadOutcome::OutOfRange => None,
    })
}

/// Positions recorded by the sub-structure walk (specification 3, 3.7.0).
#[derive(Default)]
struct Chain {
    ttxt: Option<usize>,
    igl: Option<usize>,
    trl1: Option<usize>,
    trl2: Option<usize>,
}

fn walk(z: &[u8]) -> Chain {
    let e = z.len();
    let mut chain = Chain::default();
    let mut p = SECTOR_SIZE;
    while p + 8 <= e {
        match sub(z, p, 8) {
            b"SACDTTxt" => {
                chain.ttxt.get_or_insert(p);
                p += SECTOR_SIZE;
            }
            b"SACD_IGL" => {
                chain.igl = Some(p);
                p += 2 * SECTOR_SIZE;
            }
            b"SACD_ACC" => p += 32 * SECTOR_SIZE,
            b"SACDTRL1" => {
                chain.trl1 = Some(p);
                p += SECTOR_SIZE;
            }
            b"SACDTRL2" => {
                chain.trl2 = Some(p);
                p += SECTOR_SIZE;
            }
            _ => break,
        }
    }
    chain
}

fn read_area<R: Read + Seek>(
    img: &mut Image<R>,
    slot: Slot,
    ptr: AreaPointer,
    warnings: &mut Vec<Warning>,
) -> Result<Option<Area>, Error> {
    let skip = |warnings: &mut Vec<Warning>, code| {
        warnings.push(Warning::area(code, slot));
        Ok(None)
    };
    let t = ptr.toc_sectors;
    if t > MAX_AREA_TOC_SECTORS {
        return skip(warnings, WarningCode::AreaTocTooLarge);
    }
    let toc1 = read_copy(img, ptr.toc1_lsn, t)?;
    if toc1.is_none() {
        warnings.push(Warning::area(WarningCode::AreaToc1Unreadable, slot));
    }
    let toc2 = if ptr.toc2_lsn != 0 {
        let c = read_copy(img, ptr.toc2_lsn, t)?;
        if c.is_none() {
            warnings.push(Warning::area(WarningCode::AreaToc2Unreadable, slot));
        }
        c
    } else {
        None
    };
    let (z, toc_lsn) = match (toc1, toc2) {
        (Some(a), b) => {
            if b.is_some_and(|b| b != a) {
                warnings.push(Warning::area(WarningCode::AreaTocCopiesDiffer, slot));
            }
            (a, ptr.toc1_lsn)
        }
        (None, Some(b)) => (b, ptr.toc2_lsn),
        (None, None) => return skip(warnings, WarningCode::AreaTocUnreadable),
    };
    let e = z.len();

    let kind = match sub(&z, 0, 8) {
        b"TWOCHTOC" => AreaKind::TwoChannel,
        b"MULCHTOC" => AreaKind::Multichannel,
        _ => return skip(warnings, WarningCode::AreaSignatureInvalid),
    };
    let chain = walk(&z);
    let (Some(trl1), Some(trl2)) = (chain.trl1, chain.trl2) else {
        return skip(warnings, WarningCode::AreaTrackListMissing);
    };
    let track_count = byte(&z, 69);
    if let Some(g) = chain.igl {
        if track_count >= 1 && g + 2 * SECTOR_SIZE > e {
            return skip(warnings, WarningCode::AreaTocMalformed);
        }
    }
    let audio_start_lsn = be32(&z, 72);
    let audio_end_lsn = be32(&z, 76);
    if audio_end_lsn < audio_start_lsn {
        return skip(warnings, WarningCode::AreaAudioRangeReversed);
    }

    // The area survives; its own warnings can now be reported.
    let text_charset = byte(&z, 90);
    let cs = Charset::from_code(text_charset);
    let text_channel_count = byte(&z, 80);
    let locales =
        (0..usize::from(text_channel_count.min(10))).map(|i| Locale::parse(sub(&z, 88 + 4 * i, 4))).collect();
    let mut area_text = AreaText::default();
    for (i, &item) in AreaTextItem::ALL.iter().enumerate() {
        let off = usize::from(be16(&z, 144 + 2 * i));
        if off != 0 && off < e {
            *area_text.slot(item) = text::text_string(&z, off, cs);
        }
    }
    let sample_frequency_code = byte(&z, 20);
    if sample_frequency_code != 4 {
        warnings.push(Warning::area(WarningCode::SampleFrequencyUnknown, slot));
    }
    let playtime = Timecode::parse(sub(&z, 64, 3));
    let p_frames = playtime.frame_number();

    let mut tracks = Vec::with_capacity(usize::from(track_count));
    for i in 0..usize::from(track_count) {
        // Track numbers are 1 to 255: i is at most 254.
        let number = u8::try_from(i + 1).unwrap_or(u8::MAX);
        let start_tc = Timecode::parse(sub(&z, trl2 + 8 + 4 * i, 3));
        let dur_tc = Timecode::parse(sub(&z, trl2 + 1028 + 4 * i, 3));
        let (mut start_frame, mut duration_frames) = (start_tc.frame_number(), dur_tc.frame_number());
        if start_frame + duration_frames > p_frames {
            start_frame = start_frame.min(p_frames);
            duration_frames = duration_frames.min(p_frames - start_frame);
            warnings.push(Warning::track(WarningCode::TrackPastPlaytime, slot, number));
        }
        let start_lsn = be32(&z, trl1 + 8 + 4 * i);
        if !(audio_start_lsn..=audio_end_lsn).contains(&start_lsn) {
            warnings.push(Warning::track(WarningCode::TrackLsnOutsideArea, slot, number));
        }
        let (isrc, genre) = match chain.igl {
            Some(g) => {
                let isrc = text::fixed_text(sub(&z, g + 8 + 12 * i, 12));
                ((!isrc.is_empty()).then_some(isrc), Some(Genre::parse(sub(&z, g + 3072 + 4 * i, 4))))
            }
            None => (None, None),
        };
        let text = match chain.ttxt {
            Some(b) => track_text(&z, b, i, cs, slot, number, warnings),
            None => TrackText::default(),
        };
        tracks.push(Track {
            number,
            start_lsn,
            length_sectors: be32(&z, trl1 + 1028 + 4 * i),
            start_frame,
            duration_frames,
            start_flags: byte(&z, trl2 + 11 + 4 * i),
            duration_flags: byte(&z, trl2 + 1031 + 4 * i),
            isrc,
            genre,
            text,
        });
    }

    Ok(Some(Area {
        slot,
        kind,
        version: Version { major: byte(&z, 8), minor: byte(&z, 9) },
        toc_lsn,
        toc_sectors: t,
        max_byte_rate: be32(&z, 16),
        sample_frequency_code,
        frame_format: byte(&z, 21) & 0x0F,
        channel_count: byte(&z, 32),
        area_config: byte(&z, 33),
        max_available_channels: byte(&z, 34),
        area_mute_flags: byte(&z, 35),
        track_attribute: byte(&z, 48) & 0x0F,
        playtime,
        track_offset: byte(&z, 68),
        audio_start_lsn,
        audio_end_lsn,
        text_channel_count,
        locales,
        text_charset,
        text: area_text,
        tracks,
    }))
}

/// Parses one track's text (specification 3, 3.8.1).
fn track_text(
    z: &[u8],
    b: usize,
    i: usize,
    cs: Charset,
    slot: Slot,
    number: u8,
    warnings: &mut Vec<Warning>,
) -> TrackText {
    let e = z.len();
    let mut out = TrackText::default();
    let offset = usize::from(be16(z, b + 8 + 2 * i));
    if offset == 0 {
        return out;
    }
    let mut q = b + offset;
    if q + 4 > e {
        warnings.push(Warning::track(WarningCode::TrackTextOffsetInvalid, slot, number));
        return out;
    }
    let k = byte(z, q);
    q += 4;
    let mut warned_type = false;
    for j in 0..k {
        if q + 2 > e {
            break;
        }
        let item_type = byte(z, q);
        q += 2;
        let bytes = text::string_bytes(z, q);
        let r = bytes.len();
        if r > 0 {
            match TrackTextItem::from_type(item_type) {
                Some(item) => *out.slot(item) = text::decode_string(bytes, cs),
                None if !warned_type => {
                    warned_type = true;
                    warnings.push(Warning::track(WarningCode::TrackTextTypeUnknown, slot, number));
                }
                None => {}
            }
        }
        if j + 1 < k {
            let from = q + r;
            q = z.get(from..).and_then(|rest| rest.iter().position(|&x| x != 0)).map_or(e, |n| from + n);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_text_item_types() {
        assert_eq!(TrackTextItem::from_type(0x01), Some(TrackTextItem::Title));
        assert_eq!(TrackTextItem::from_type(0x07), Some(TrackTextItem::ExtraMessage));
        assert_eq!(TrackTextItem::from_type(0x81), Some(TrackTextItem::TitlePhonetic));
        assert_eq!(TrackTextItem::from_type(0x87), Some(TrackTextItem::ExtraMessagePhonetic));
        for t in [0x00, 0x08, 0x41, 0x80, 0x88, 0xC1, 0xFF] {
            assert_eq!(TrackTextItem::from_type(t), None, "{t:#x}");
        }
    }

    #[test]
    fn timecode_frame_numbers() {
        let tc = Timecode { minutes: 255, seconds: 255, frames: 255 };
        assert_eq!(tc.frame_number(), 1_166_880);
        assert_eq!(Timecode { minutes: 0, seconds: 1, frames: 2 }.frame_number(), 77);
    }

    #[test]
    fn walk_stops_and_records() {
        let mut z = vec![0u8; 6 * SECTOR_SIZE];
        z[2048..2056].copy_from_slice(b"SACDTTxt");
        z[4096..4104].copy_from_slice(b"SACDTTxt");
        z[6144..6152].copy_from_slice(b"SACDTRL1");
        z[8192..8200].copy_from_slice(b"SACDTRL2");
        z[10240..10248].copy_from_slice(b"SACD_IGL");
        let c = walk(&z);
        assert_eq!((c.ttxt, c.trl1, c.trl2, c.igl), (Some(2048), Some(6144), Some(8192), Some(10240)));
        z[4096..4104].copy_from_slice(b"XXXXXXXX");
        let c = walk(&z);
        assert_eq!((c.ttxt, c.trl1), (Some(2048), None));
    }
}
