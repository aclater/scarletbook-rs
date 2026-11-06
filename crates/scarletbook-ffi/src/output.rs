// SPDX-License-Identifier: MIT OR Apache-2.0

//! DSF and DSDIFF output (specification 8; issue #131): output options, tag items, DST
//! frame lengths and layouts, over `scarletbook::output`.

use std::borrow::Cow;
use std::cell::RefCell;
use std::ffi::{CStr, c_char};

use scarletbook::Error;
use scarletbook::output::{
    AlbumArtistFrame, ArrangerFrame, ArtistTitleSource, CallerChunk, Comment, CommentKind, DateFrame,
    DsdiffDeparture, DsfBitsPerSample, DstIndexOffset, DstLengths, Format, GenreSources, Id3Encoding,
    InfoChunk, Input, InvalidDst, Layout, LocalChunk, Marker, MarkerKind, MarkerText, Markers, OutputOptions,
    PositionSource, PropertyChunk, RegionKind, SourceTrack, TagItem, TagSet, Tags, TextSource,
    plan_dsf_chain,
};

use super::{
    Failure, REENTRANT, SbDisc, SbStatus, clear_out, copy_string, disc_handle, guard, slot_from, slot_index,
    write_out,
};

/// Output file formats, as passed in `format` arguments.
#[repr(u32)]
#[derive(Clone, Copy, Debug)]
pub enum SbOutputFormat {
    /// DSF with the tag of the options (specification 8, 8.2).
    Dsf = 0,
    /// DSDIFF with plain DSD sound data (8.4.5).
    DsdiffDsd = 1,
    /// DSDIFF with DST sound data (8.4.6); made with `sb_output_layout_new_dst`.
    DsdiffDst = 2,
}

/// The integer-valued output options of `sb_output_options_set`: specification 8, 8.6's
/// writer's choices (W numbers). Each value's meaning and the default are given per option.
#[repr(u32)]
#[derive(Clone, Copy, Debug)]
pub enum SbOutputOption {
    /// W1: 1 (bits reversed, the default) or 8 (unchanged).
    DsfBitsPerSample = 0,
    /// W2: 0 refuses an unspecified layout (the default), or a DSF channel type 1 to 7
    /// with the area's channel count.
    DsfUnspecifiedChannelType = 1,
    /// W15: 1 writes `LSCO` 65 535 for an unspecified layout; 0 (the default) none.
    DsdiffUnspecifiedLoudspeakerConfiguration = 2,
    /// W16: 1 (the default) writes `LSCO` for a defined layout; 0 none.
    DsdiffLoudspeakerConfiguration = 3,
    /// W17: the absolute start frame `A`, or -1 (the default) for no `ABSS`.
    DsdiffAbsoluteStart = 4,
    /// W19: 1 writes a `DSTC` frame CRC after each `DSTF`; 0 (the default) none.
    DsdiffDstCrc = 5,
    /// W20: 1 (the default) writes the `DSTI` sound index; 0 none.
    DsdiffDstIndex = 6,
    /// W21: 0 (the default) for `DSTI` offsets at the `DSTF` chunk header, 1 at its frame
    /// data.
    DsdiffDstIndexOffset = 7,
    /// W29: -1 (the default) finds the source track; 0 none; 1 to 255 that track.
    SourceTrack = 8,
    /// 1 writes an edited master (8.4.11); 0 (the default) does not.
    DsdiffEditedMaster = 9,
    /// W24: 1 writes `DIIN`; 0 (the default) does not (an edited master always has one).
    DsdiffEditedMasterInfo = 10,
    /// W28: 1 gives a file with a source track its one-track program; 0 (the default) not.
    DsdiffSourceTrackMarkers = 11,
    /// W30: 1 gives a frame range without a source track its clipped program; 0 (the
    /// default) not.
    DsdiffClippedProgram = 12,
    /// W22: 1 counts the pad byte of odd text in `D`; 0 (the default) not.
    DsdiffTextPadCounted = 13,
    /// W24: -1 (the default) writes `DIAR` and `DITI` in an edited master only; 0 never;
    /// 1 whenever `DIIN` is written.
    DsdiffArtistTitle = 14,
    /// W23: 0 (the default) for markers without text, 1 for the track title on track
    /// starts.
    DsdiffMarkerText = 15,
    /// W26: -1 removes other code points from DSDIFF text; a byte 0x20 to 0x7E replaces
    /// them (the default `?`).
    DsdiffTextReplacement = 16,
    /// W31: 1 writes empty DSDIFF text with `k = 0`; 0 (the default) leaves it out.
    DsdiffEmptyText = 17,
    /// W3: 0 (the default) tags a DSF file from the TOC, 1 writes no tag (a tag set or raw
    /// tag is set with `sb_output_options_set_tags` or `sb_output_options_set_raw_tag`).
    Tags = 18,
    /// W11: 0 the disc's text, 1 the album's, 2 (the default) the disc's or else the
    /// album's.
    TextSource = 19,
    /// W11: the genre sources, a bit mask: 1 the track's, 2 the album's, 4 the disc's (the
    /// default 7).
    Genres = 20,
    /// W4: 1 (the default) writes an ID3 footer; 0 none.
    Id3Footer = 21,
    /// W5: ID3 padding bytes without a footer (default 0).
    Id3Padding = 22,
    /// W7: 0 UTF-8 (the default), 1 ISO 8859-1 where allowed, else UTF-8, 2 UTF-16 LE with
    /// BOM, 3 UTF-16 BE with BOM, 4 UTF-16BE without.
    Id3Encoding = 23,
    /// W8: 1 terminates the last string of text frames; 0 (the default) not.
    Id3TerminateLast = 24,
    /// W9: -1 (the default) removes control characters; a code point replaces them.
    Id3ControlReplacement = 25,
    /// W11: 0 (the default) the arranger in `TIPL`, 1 in `TXXX`.
    Id3Arranger = 26,
    /// W11: 1 writes `TSOT` and `TSOP`; 0 (the default) not.
    Id3Phonetic = 27,
    /// W11: 1 (the default) writes `TRCK` with `/count`; 0 without.
    Id3TrackCount = 28,
    /// W11: 0 (the default) no album-artist frame, 1 `TPE2`, 2 `TXXX`.
    Id3AlbumArtist = 29,
    /// W11: 1 (the default) writes `TCOP`; 0 not.
    Id3Copyright = 30,
    /// W11: 1 writes a sequence number outside the set alone in `TPOS`; 0 (the default)
    /// not.
    Id3AlbumSetAlone = 31,
    /// W11: 0 (the default) the date in `TDRL`, 1 in `TDRC`.
    Id3Date = 32,
    /// W24 (specification 8, 8.6 row 24 and 8.4.8, sacd-spec e069569): 1 (the default)
    /// writes `EMID` in `DIIN`; 0 leaves it out of a file that is not an edited master. An
    /// edited master requires `EMID` (8.4.11 item 1), so 0 makes one
    /// `SB_STATUS_INVALID_ARGUMENT`. A `DIIN` left with none of its chunks is written or
    /// left out as `SB_OUTPUT_OPTION_DSDIFF_EMPTY_INFO` says.
    DsdiffEditedMasterIdWritten = 33,
    /// W24 (specification 8, 8.6 row 24 and 8.4.8, sacd-spec e069569): what fills `DIAR`
    /// and `DITI` when they are written: 0 (the default) the album artist and album items;
    /// 1 the source track's performer and title items (specification 3, 3.8, types 0x02 and
    /// 0x01), none in a file without a source track; 2 each of those, or else the album
    /// item.
    DsdiffArtistTitleSource = 34,
    /// Departure D1 of specification 8, 8.7 (sacd-spec `e069569`): 1 writes a plain DSD
    /// file's `CMPR` with `D = 20`, counting its pad byte, instead of 19; 0 (the default)
    /// does not. The bytes are the same apart from `D`; DST files are unchanged. **A file
    /// written with 1 is not a conforming DSDIFF file.**
    DsdiffDepartureD1 = 35,
    /// Departure D2 of specification 8, 8.7 (sacd-spec `e069569`): 1 writes `DIAR`, `DITI`,
    /// marker and comment text as ISO 8859-1, U+0020 to U+007E and U+00A0 to U+00FF as one
    /// byte each and every other code point removed (`SB_OUTPUT_OPTION_DSDIFF_TEXT_REPLACEMENT`
    /// is then ignored), with `k` counting the bytes written; 0 (the default) does not. `EMID` stays
    /// bytes 0x20 to 0x7E. **A file written with 1 is not a conforming DSDIFF file.**
    DsdiffDepartureD2 = 36,
    /// W32 (specification 8, 8.6 row 32; 8.2.7, sacd-spec e069569): -1 (the default) for a
    /// DSF file holding its range of positions; otherwise, with
    /// `SB_OUTPUT_OPTION_DSF_CHAIN_END`, the file holds the byte range of its range
    /// `[a_i, b_i)` in a chain whose first range starts at this frame `a_1`.
    /// `sb_output_dsf_chain_plan` gives both values per file. The byte range is always one
    /// of 8.2.7 item 3's `[p_i, p_{i+1})` (sacd-spec fa558a8, item 2), that of the chain
    /// `[a_1, a_i)`, `[a_i, b_i)`, `[b_i, b_K)`. Both are set or neither,
    /// with `a_1 <= a_i < b_i <= b_K` and `[b_i, b_K)` passing the checks of 8.1 item 2
    /// (`sb_range_check`), DSF only; otherwise the layout is `SB_STATUS_INVALID_ARGUMENT`.
    DsfChainFirst = 37,
    /// W32: -1 (the default), or the end `b_K` of the chain's last range (8.2.7 item 3);
    /// the file whose range ends there is the last of the chain. See
    /// `SB_OUTPUT_OPTION_DSF_CHAIN_FIRST`.
    DsfChainEnd = 38,
    /// W34 (specification 8, 8.6 row 34 and 8.4.8, sacd-spec 048aa1a): 1 (the default)
    /// writes a `DIIN` that would hold none of its chunks (no `EMID`, markers, `DIAR` or
    /// `DITI`; only outside an edited master) with `D = 0`, 12 bytes; 0 leaves it out.
    DsdiffEmptyInfo = 39,
}

/// The string-valued output options of `sb_output_options_set_string`.
#[repr(u32)]
#[derive(Clone, Copy, Debug)]
pub enum SbOutputString {
    /// W24: the text of `EMID` (default empty); text bytes 0x20 to 0x7E.
    DsdiffEditedMasterId = 0,
    /// W10: a separator at which ID3 values are split; NULL (the default) for none.
    Id3Split = 1,
    /// W11, W12: the separator joining both messages in one `COMM`; NULL (the default)
    /// for one frame each.
    Id3JoinMessages = 2,
    /// W12: the `COMM` language, three lower-case letters or `XXX` (the default).
    Id3Language = 3,
    /// W12: the description of the message's `COMM` (default empty).
    Id3MessageDescription = 4,
    /// W12: the description of the extra message's `COMM` (default `extra message`).
    Id3ExtraMessageDescription = 5,
    /// W12: the `TXXX` description of the arranger (default `arranger`).
    Id3ArrangerDescription = 6,
    /// W12: the `TXXX` description of the album artist (default `album artist`).
    Id3AlbumArtistDescription = 7,
}

/// The chunk orders of `sb_output_options_set_order`.
#[repr(u32)]
#[derive(Clone, Copy, Debug)]
pub enum SbOutputOrder {
    /// W14, inside `PROP`: 0 `FS  `, 1 `CHNL`, 2 `CMPR`, 3 `ABSS`, 4 `LSCO`, each once
    /// (the default 0 1 2 3 4).
    Property = 0,
    /// W13, the local chunks after `FVER`: 0 `PROP`, 1 the sound data, 2 `DSTI`, 3 `DIIN`,
    /// 4 `COMT`, each the file holds once, `PROP` before the sound data (the default 0 4
    /// 3 1 2).
    Local = 1,
    /// W14, inside `DIIN`: 0 `EMID`, 1 the markers, 2 `DIAR`, 3 `DITI`, each once (the
    /// default 0 1 2 3); chunks that aren't written are skipped.
    Info = 2,
}

/// Tag items (specification 8, 8.3.5), as passed in `item` arguments.
#[repr(u32)]
#[derive(Clone, Copy, Debug)]
pub enum SbTagItem {
    /// Track title.
    TrackTitle = 0,
    /// Track performer.
    Performer = 1,
    /// Songwriter.
    Songwriter = 2,
    /// Composer.
    Composer = 3,
    /// Arranger.
    Arranger = 4,
    /// Message.
    Message = 5,
    /// Extra message.
    ExtraMessage = 6,
    /// Track title, phonetic.
    TitlePhonetic = 7,
    /// Track performer, phonetic.
    PerformerPhonetic = 8,
    /// The source track's number.
    TrackNumber = 9,
    /// The area's track count.
    TrackCount = 10,
    /// ISRC.
    Isrc = 11,
    /// Genre names.
    Genre = 12,
    /// Album or disc title.
    Album = 13,
    /// Album or disc artist.
    AlbumArtist = 14,
    /// Album or disc publisher.
    Publisher = 15,
    /// Album or disc copyright.
    Copyright = 16,
    /// Album sequence number.
    DiscNumber = 17,
    /// Album set size.
    DiscCount = 18,
    /// Date, `yyyy-MM-dd`, `yyyy-MM` or `yyyy`.
    Date = 19,
}

/// What a layout's `sb_output_layout_fill` needs per position (`SbOutputLayoutInfo`).
#[repr(u32)]
#[derive(Clone, Copy, Debug)]
pub enum SbOutputInput {
    /// The content (decoded, `C x 4704` bytes).
    Content = 0,
    /// The DST equivalents.
    DstEquivalents = 1,
    /// The DST equivalents, and the content for the frame CRCs.
    DstEquivalentsAndContent = 2,
}

/// What a region of an output file holds.
#[repr(u32)]
#[derive(Clone, Copy, Debug)]
pub enum SbOutputRegionKind {
    /// Bytes before the sound data.
    Header = 0,
    /// The sound data.
    SoundData = 1,
    /// Chunks after the sound data, or the DSF tag.
    Trailer = 2,
}

/// A DSDIFF marker for `sb_output_options_add_marker` (specification 8, 8.4.7).
///
/// Layout: `text` at offset 0, then four `uint32_t`; 24 bytes on 64-bit targets, 20 on
/// 32-bit ones, without padding.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SbMarker {
    /// NUL-terminated UTF-8 text, or NULL for none. Each code point is written as the one
    /// byte of the same value: U+0020 to U+007E, or with departure D2
    /// (`SB_OUTPUT_OPTION_DSDIFF_DEPARTURE_D2`) also U+00A0 to U+00FF. A code point above
    /// U+00FF is refused by `sb_output_options_add_marker`; others when a layout is made.
    pub text: *const c_char,
    /// The frame of the area's timeline, from the range's first to its end.
    pub frame: u32,
    /// 0 track start, 1 track stop, 2 program start, 4 index.
    pub kind: u32,
    /// Track flags (0 to 65 535), written on a track start only.
    pub track_flags: u32,
    /// 0.
    pub reserved: u32,
}

/// A DSDIFF comment for `sb_output_options_add_comment` (specification 8, 8.4.8).
///
/// Layout: `text` at offset 0, three `uint32_t`, then four `uint8_t`; 24 bytes on 64-bit
/// targets, 20 on 32-bit ones, without padding.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SbComment {
    /// NUL-terminated UTF-8 text, written as text bytes (8.4.9); NULL for empty.
    pub text: *const c_char,
    /// Year, 0 to 65 535.
    pub year: u32,
    /// 0 general, 1 channel, 2 sound source, 3 file history.
    pub kind: u32,
    /// The reference of its kind (8.4.8), 0 to 65 535.
    pub reference: u32,
    /// Month, 0 to 12.
    pub month: u8,
    /// Day, 0 to 31.
    pub day: u8,
    /// Hour, 0 to 23.
    pub hour: u8,
    /// Minutes, 0 to 59.
    pub minutes: u8,
}

/// The content a caller holds of one position, for `sb_output_layout_fill`.
///
/// Layout: two pointer and length pairs, then two `uint32_t`; 40 bytes on 64-bit targets,
/// 24 on 32-bit ones, without padding.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SbPositionData {
    /// Its content (`C x 4704` bytes), or NULL when the layout doesn't need it.
    pub content: *const u8,
    /// The content's length.
    pub content_len: usize,
    /// Its DST equivalent as the file holds it, or NULL when the layout doesn't need it.
    pub dst_equivalent: *const u8,
    /// The DST equivalent's length.
    pub dst_equivalent_len: usize,
    /// The position's frame number.
    pub number: u32,
    /// 0.
    pub reserved: u32,
}

/// A layout's description (`sb_output_layout_info`).
///
/// Layout: 48 bytes, aligned to 8, without padding (2 fields of 8 bytes, then 8 of 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SbOutputLayoutInfo {
    /// The file's size in bytes.
    pub size: u64,
    /// The memory the layout holds, in bytes.
    pub memory: u64,
    /// The format (`SbOutputFormat`).
    pub format: u32,
    /// The area's slot.
    pub slot: u32,
    /// The range's first frame.
    pub first: u32,
    /// The range's end frame (exclusive).
    pub end: u32,
    /// The area's channel count.
    pub channels: u32,
    /// The source track's number, or 0.
    pub source_track: u32,
    /// What `sb_output_layout_fill` needs (`SbOutputInput`).
    pub input: u32,
    /// The number of regions.
    pub regions: u32,
}

/// A region of an output file (`sb_output_layout_region`).
///
/// Layout: 24 bytes, aligned to 8, without padding.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SbOutputRegion {
    /// Its first byte's offset.
    pub offset: u64,
    /// Its length.
    pub len: u64,
    /// What it holds (`SbOutputRegionKind`).
    pub kind: u32,
    /// 0.
    pub reserved: u32,
}

/// Output options: the writer's choices of specification 8, 8.6.
#[derive(Debug)]
pub struct SbOutputOptions {
    inner: RefCell<OutputOptions>,
}

/// Tag items of a file (specification 8, 8.3.5).
#[derive(Debug)]
pub struct SbTags {
    inner: RefCell<TagSet>,
}

/// The lengths of a range's DST equivalents (specification 8, 8.4.6 "Sizes").
#[derive(Debug)]
pub struct SbDstLengths {
    /// Borrowed for each call, so a callback that frees the handle mid-call is refused.
    inner: RefCell<DstLengths>,
}

/// The layout of one output file.
#[derive(Debug)]
pub struct SbOutputLayout {
    /// Borrowed for each call, so a callback that frees the handle mid-call (a read's
    /// read or log callback) is refused.
    inner: RefCell<Layout>,
}

const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<SbOutputOptions>();
    assert_send::<SbTags>();
    assert_send::<SbDstLengths>();
    assert_send::<SbOutputLayout>();
};

/// Borrows a handle.
///
/// # Safety
///
/// `p` must be NULL or a live handle of its type.
unsafe fn handle<'a, T>(p: *const T, what: &str) -> Result<&'a T, Failure> {
    // SAFETY: NULL or a live handle by this function's contract; only shared references
    // to it are ever made.
    unsafe { p.as_ref() }.ok_or_else(|| Failure::arg(&format!("NULL {what} handle")))
}

/// A slice from a pointer and a count: NULL is allowed for 0 elements.
///
/// # Safety
///
/// `p` must be NULL or valid for reads of `n` elements.
unsafe fn slice<'a, T>(p: *const T, n: usize) -> Result<&'a [T], Failure> {
    if n == 0 {
        return Ok(&[]);
    }
    if p.is_null() {
        return Err(Failure::arg("NULL array with a non-zero count"));
    }
    // SAFETY: non-NULL and valid for n reads by this function's contract.
    Ok(unsafe { std::slice::from_raw_parts(p, n) })
}

/// A NUL-terminated UTF-8 string, or `None` for NULL.
///
/// # Safety
///
/// `s` must be NULL or a NUL-terminated string.
unsafe fn string(s: *const c_char) -> Result<Option<String>, Failure> {
    if s.is_null() {
        return Ok(None);
    }
    // SAFETY: non-NULL and NUL-terminated by this function's contract.
    let s = unsafe { CStr::from_ptr(s) };
    s.to_str().map(|s| Some(s.to_owned())).map_err(|_| Failure::arg("a string is not UTF-8"))
}

/// Frees a handle made by `Box::into_raw`, unless it is in use.
///
/// # Safety
///
/// `p` must be NULL or a live handle, freed only once.
unsafe fn free<T>(p: *mut T, in_use: impl FnOnce(&T) -> bool) {
    let _ = guard(|| {
        // SAFETY: forwarded contract.
        let Some(h) = (unsafe { p.as_ref() }) else { return Ok(SbStatus::Ok) };
        if in_use(h) {
            return Err(Failure::arg(REENTRANT));
        }
        // SAFETY: a live handle created by Box::into_raw and not in use, freed only once.
        drop(unsafe { Box::from_raw(p) });
        Ok(SbStatus::Ok)
    });
}

/// Writes a new handle to `out`.
///
/// # Safety
///
/// `out` must be valid for a write (checked non-NULL by the caller).
unsafe fn give<T>(out: *mut *mut T, value: T) {
    // SAFETY: non-NULL and valid for a write by the caller's contract.
    unsafe { out.write(Box::into_raw(Box::new(value))) };
}

fn non_null<T>(out: *mut *mut T) -> Result<(), Failure> {
    if out.is_null() { Err(Failure::arg("NULL output pointer")) } else { Ok(()) }
}

fn tag_item(item: u32) -> Result<TagItem, Failure> {
    use TagItem::*;
    const ITEMS: [TagItem; 20] = [
        TrackTitle,
        Performer,
        Songwriter,
        Composer,
        Arranger,
        Message,
        ExtraMessage,
        TitlePhonetic,
        PerformerPhonetic,
        TrackNumber,
        TrackCount,
        Isrc,
        Genre,
        Album,
        AlbumArtist,
        Publisher,
        Copyright,
        DiscNumber,
        DiscCount,
        Date,
    ];
    ITEMS.get(item as usize).copied().ok_or_else(|| Failure::arg("unknown tag item"))
}

fn flag(value: i64) -> Result<bool, Failure> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(Failure::arg("the option takes 0 or 1")),
    }
}

fn choice<T: Copy>(value: i64, values: &[T]) -> Result<T, Failure> {
    usize::try_from(value)
        .ok()
        .and_then(|i| values.get(i).copied())
        .ok_or_else(|| Failure::arg("unknown option value"))
}

/// Makes departure `d` (specification 8, 8.7) or not.
fn departure(o: &mut OutputOptions, d: DsdiffDeparture, on: bool) {
    if on {
        o.dsdiff_departures.insert(d);
    } else {
        o.dsdiff_departures.remove(&d);
    }
}

/// Applies one integer option.
fn set_option(o: &mut OutputOptions, option: u32, value: i64) -> Result<(), Failure> {
    let u32_of = |v: i64| u32::try_from(v).map_err(|_| Failure::arg("the option's value is out of range"));
    match option {
        0 => {
            o.dsf_bits_per_sample = match value {
                1 => DsfBitsPerSample::One,
                8 => DsfBitsPerSample::Eight,
                _ => return Err(Failure::arg("DSF bits per sample are 1 or 8")),
            }
        }
        1 => o.dsf_unspecified_channel_type = if value == 0 { None } else { Some(u32_of(value)?) },
        2 => o.dsdiff_unspecified_loudspeaker_configuration = flag(value)?,
        3 => o.dsdiff_loudspeaker_configuration = flag(value)?,
        4 => o.dsdiff_absolute_start = if value == -1 { None } else { Some(u32_of(value)?) },
        5 => o.dsdiff_dst_crc = flag(value)?,
        6 => o.dsdiff_dst_index = flag(value)?,
        7 => {
            o.dsdiff_dst_index_offset =
                choice(value, &[DstIndexOffset::ChunkHeader, DstIndexOffset::FrameData])?
        }
        8 => {
            o.source_track = match value {
                -1 => SourceTrack::Auto,
                0 => SourceTrack::None,
                n => SourceTrack::Track(
                    u8::try_from(n).map_err(|_| Failure::arg("track numbers are 1 to 255"))?,
                ),
            }
        }
        9 => o.dsdiff_edited_master = flag(value)?,
        10 => o.dsdiff_edited_master_info = flag(value)?,
        11 => o.dsdiff_source_track_markers = flag(value)?,
        12 => o.dsdiff_clipped_program = flag(value)?,
        13 => o.dsdiff_text_pad_counted = flag(value)?,
        14 => o.dsdiff_artist_title = if value == -1 { None } else { Some(flag(value)?) },
        15 => o.dsdiff_marker_text = choice(value, &[MarkerText::Empty, MarkerText::TrackTitle])?,
        16 => {
            o.dsdiff_text_replacement = match value {
                -1 => None,
                0x20..=0x7E => Some(value as u8),
                _ => return Err(Failure::arg("the replacement is a text byte 0x20 to 0x7E")),
            }
        }
        17 => o.dsdiff_empty_text = flag(value)?,
        18 => o.tags = if flag(value)? { Tags::None } else { Tags::FromToc },
        19 => {
            o.text_source = choice(value, &[TextSource::Disc, TextSource::Album, TextSource::DiscThenAlbum])?
        }
        20 => {
            if !(0..=7).contains(&value) {
                return Err(Failure::arg("genre sources are a mask of 1, 2 and 4"));
            }
            o.genres = GenreSources::new()
                .with_track(value & 1 != 0)
                .with_album(value & 2 != 0)
                .with_disc(value & 4 != 0);
        }
        21 => o.id3.footer = flag(value)?,
        22 => o.id3.padding = u32_of(value)?,
        23 => {
            o.id3.encoding = choice(
                value,
                &[
                    Id3Encoding::Utf8,
                    Id3Encoding::Latin1OrUtf8,
                    Id3Encoding::Utf16Le,
                    Id3Encoding::Utf16Be,
                    Id3Encoding::Utf16BeNoBom,
                ],
            )?
        }
        24 => o.id3.terminate_last = flag(value)?,
        25 => {
            o.id3.control_replacement = if value == -1 {
                None
            } else {
                Some(
                    u32_of(value)
                        .ok()
                        .and_then(char::from_u32)
                        .ok_or_else(|| Failure::arg("not a code point"))?,
                )
            }
        }
        26 => o.id3.arranger = choice(value, &[ArrangerFrame::Tipl, ArrangerFrame::Txxx])?,
        27 => o.id3.phonetic = flag(value)?,
        28 => o.id3.track_count = flag(value)?,
        29 => {
            o.id3.album_artist =
                choice(value, &[AlbumArtistFrame::None, AlbumArtistFrame::Tpe2, AlbumArtistFrame::Txxx])?
        }
        30 => o.id3.copyright = flag(value)?,
        31 => o.id3.album_set_alone = flag(value)?,
        32 => o.id3.date = choice(value, &[DateFrame::Tdrl, DateFrame::Tdrc])?,
        33 => o.dsdiff_edited_master_id_written = flag(value)?,
        34 => {
            o.dsdiff_artist_title_source = choice(
                value,
                &[ArtistTitleSource::Album, ArtistTitleSource::Track, ArtistTitleSource::TrackThenAlbum],
            )?
        }
        35 => departure(o, DsdiffDeparture::D1, flag(value)?),
        36 => departure(o, DsdiffDeparture::D2, flag(value)?),
        37 => o.dsf_chain_first = if value == -1 { None } else { Some(u32_of(value)?) },
        38 => o.dsf_chain_end = if value == -1 { None } else { Some(u32_of(value)?) },
        39 => o.dsdiff_empty_info = flag(value)?,
        _ => return Err(Failure::arg("unknown output option")),
    }
    Ok(())
}

/// Makes output options with their defaults (specification 8, 8.6, as documented per
/// option). On failure `*out` is set to NULL.
///
/// # Safety
///
/// `out` must be NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_output_options_new(out: *mut *mut SbOutputOptions) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { clear_out(out) };
    guard(|| {
        non_null(out)?;
        // SAFETY: non-NULL, and valid for a write by this function's contract.
        unsafe { give(out, SbOutputOptions { inner: RefCell::new(OutputOptions::default()) }) };
        Ok(SbStatus::Ok)
    })
}

/// Frees output options. NULL is ignored.
///
/// # Safety
///
/// `options` must be NULL or a live handle, freed only once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_output_options_free(options: *mut SbOutputOptions) {
    // SAFETY: forwarded contract.
    unsafe { free(options, |h| h.inner.try_borrow_mut().is_err()) };
}

/// Sets the integer option `option` (`SbOutputOption`) to `value`, as documented per
/// option. `SB_STATUS_INVALID_ARGUMENT` for an unknown option or value.
///
/// # Safety
///
/// `options` must be NULL or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_output_options_set(
    options: *const SbOutputOptions,
    option: u32,
    value: i64,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let h = unsafe { handle(options, "output options") }?;
        let mut o = h.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?;
        set_option(&mut o, option, value)?;
        Ok(SbStatus::Ok)
    })
}

/// Sets the string option `which` (`SbOutputString`) to the NUL-terminated UTF-8 string
/// `value`; NULL sets the options that allow it to none, and the others to their
/// defaults.
///
/// # Safety
///
/// `options` must be NULL or a live handle; `value` NULL or a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_output_options_set_string(
    options: *const SbOutputOptions,
    which: u32,
    value: *const c_char,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let h = unsafe { handle(options, "output options") }?;
        // SAFETY: forwarded contract.
        let v = unsafe { string(value) }?;
        let mut o = h.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?;
        let d = OutputOptions::default();
        match which {
            0 => o.dsdiff_edited_master_id = v.unwrap_or_default(),
            1 => o.id3.split = v,
            2 => o.id3.join_messages = v,
            3 => {
                o.id3.language = match v {
                    None => d.id3.language,
                    Some(s) => {
                        s.as_bytes().try_into().map_err(|_| Failure::arg("a language is three letters"))?
                    }
                }
            }
            4 => o.id3.message_description = v.unwrap_or(d.id3.message_description),
            5 => o.id3.extra_message_description = v.unwrap_or(d.id3.extra_message_description),
            6 => o.id3.arranger_description = v.unwrap_or(d.id3.arranger_description),
            7 => o.id3.album_artist_description = v.unwrap_or(d.id3.album_artist_description),
            _ => return Err(Failure::arg("unknown output string option")),
        }
        Ok(SbStatus::Ok)
    })
}

/// Sets the chunk order `which` (`SbOutputOrder`) to the `count` chunk numbers of
/// `items`, as documented per order.
///
/// # Safety
///
/// `options` must be NULL or a live handle; `items` NULL or valid for `count` reads.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_output_options_set_order(
    options: *const SbOutputOptions,
    which: u32,
    items: *const u32,
    count: usize,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let h = unsafe { handle(options, "output options") }?;
        // SAFETY: forwarded contract.
        let items = unsafe { slice(items, count) }?;
        let mut o = h.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?;
        match which {
            0 => {
                use PropertyChunk::*;
                let all =
                    [SampleRate, Channels, CompressionType, AbsoluteStartTime, LoudspeakerConfiguration];
                let v: Vec<PropertyChunk> =
                    items.iter().map(|&i| choice(i64::from(i), &all)).collect::<Result<_, _>>()?;
                o.dsdiff_property_order =
                    v.try_into().map_err(|_| Failure::arg("give the 5 property chunks"))?;
            }
            1 => {
                use LocalChunk::*;
                let all = [Property, SoundData, DstIndex, EditedMasterInfo, Comments];
                o.dsdiff_local_order =
                    items.iter().map(|&i| choice(i64::from(i), &all)).collect::<Result<_, _>>()?;
            }
            2 => {
                use InfoChunk::*;
                let all = [EditedMasterId, Markers, Artist, Title];
                let v: Vec<InfoChunk> =
                    items.iter().map(|&i| choice(i64::from(i), &all)).collect::<Result<_, _>>()?;
                o.dsdiff_info_order = v.try_into().map_err(|_| Failure::arg("give the 4 DIIN chunks"))?;
            }
            _ => return Err(Failure::arg("unknown chunk order")),
        }
        Ok(SbStatus::Ok)
    })
}

/// Sets the ID3 frame order (W6) to the `count` frame IDs of `ids`, 4 bytes each with no
/// separator (for example `"TIT2TPE1"`); frames with other IDs follow in the order they
/// are made.
///
/// # Safety
///
/// `options` must be NULL or a live handle; `ids` NULL or valid for `4 x count` reads.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_output_options_set_id3_frame_order(
    options: *const SbOutputOptions,
    ids: *const c_char,
    count: usize,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let h = unsafe { handle(options, "output options") }?;
        let n = count.checked_mul(4).ok_or_else(|| Failure::arg("too many frame IDs"))?;
        // SAFETY: forwarded contract.
        let bytes = unsafe { slice(ids.cast::<u8>(), n) }?;
        let order = bytes.chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect();
        h.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?.id3.frame_order = order;
        Ok(SbStatus::Ok)
    })
}

/// Sets the DSDIFF channel numbers (0 to 999, written `C000` to `C999`) of an area whose
/// layout is unspecified (W15); NULL and 0 restore the default `C000`, `C001`, ...
///
/// # Safety
///
/// `options` must be NULL or a live handle; `numbers` NULL or valid for `count` reads.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_output_options_set_channel_numbers(
    options: *const SbOutputOptions,
    numbers: *const u16,
    count: usize,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let h = unsafe { handle(options, "output options") }?;
        // SAFETY: forwarded contract.
        let numbers = unsafe { slice(numbers, count) }?;
        h.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?.dsdiff_unspecified_channel_numbers =
            (!numbers.is_empty()).then(|| numbers.to_vec());
        Ok(SbStatus::Ok)
    })
}

/// Adds a DSDIFF comment (W25). Its fields are checked when a layout is made.
///
/// # Safety
///
/// `options` must be NULL or a live handle; `comment` NULL or valid for a read, with
/// `text` NULL or NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_output_options_add_comment(
    options: *const SbOutputOptions,
    comment: *const SbComment,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let h = unsafe { handle(options, "output options") }?;
        // SAFETY: forwarded contract.
        let c = unsafe { comment.as_ref() }.ok_or_else(|| Failure::arg("NULL comment"))?;
        let reference = u16::try_from(c.reference).map_err(|_| Failure::arg("a reference is 0 to 65 535"))?;
        let year = u16::try_from(c.year).map_err(|_| Failure::arg("a year is 0 to 65 535"))?;
        let kind = match c.kind {
            0 => CommentKind::General,
            1 => CommentKind::Channel(reference),
            2 => CommentKind::SoundSource(reference),
            3 => CommentKind::FileHistory(reference),
            _ => return Err(Failure::arg("comment kinds are 0 to 3")),
        };
        // SAFETY: forwarded contract.
        let text = unsafe { string(c.text) }?.unwrap_or_default();
        let mut comment = Comment::new(kind, &text);
        (comment.year, comment.month, comment.day, comment.hour, comment.minutes) =
            (year, c.month, c.day, c.hour, c.minutes);
        h.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?.dsdiff_comments.push(comment);
        Ok(SbStatus::Ok)
    })
}

/// Removes every DSDIFF comment.
///
/// # Safety
///
/// `options` must be NULL or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_output_options_clear_comments(options: *const SbOutputOptions) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let h = unsafe { handle(options, "output options") }?;
        h.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?.dsdiff_comments.clear();
        Ok(SbStatus::Ok)
    })
}

/// Adds a marker of the caller's own: the file's markers are then the ones added, in
/// order, instead of the TOC's (specification 8, 8.4.10). They are checked when a layout
/// is made.
///
/// # Safety
///
/// `options` must be NULL or a live handle; `marker` NULL or valid for a read, with `text`
/// NULL or NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_output_options_add_marker(
    options: *const SbOutputOptions,
    marker: *const SbMarker,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let h = unsafe { handle(options, "output options") }?;
        // SAFETY: forwarded contract.
        let m = unsafe { marker.as_ref() }.ok_or_else(|| Failure::arg("NULL marker"))?;
        let kind = match m.kind {
            0 => MarkerKind::TrackStart,
            1 => MarkerKind::TrackStop,
            2 => MarkerKind::ProgramStart,
            4 => MarkerKind::Index,
            _ => return Err(Failure::arg("marker kinds are 0, 1, 2 and 4")),
        };
        let mut marker = Marker::new(m.frame, kind);
        marker.track_flags =
            u16::try_from(m.track_flags).map_err(|_| Failure::arg("track flags are 0 to 65 535"))?;
        // SAFETY: forwarded contract.
        let text = unsafe { string(m.text) }?.unwrap_or_default();
        // One byte per code point: the same bytes for text bytes, and ISO 8859-1 for
        // departure D2 (specification 8, 8.7). Other bytes are refused when a layout is made.
        marker.text = text
            .chars()
            .map(|c| u8::try_from(c).map_err(|_| Failure::arg("marker text is U+0000 to U+00FF")))
            .collect::<Result<_, _>>()?;
        let mut o = h.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?;
        match &mut o.dsdiff_markers {
            Markers::Custom(list) => list.push(marker),
            m => *m = Markers::Custom(vec![marker]),
        }
        Ok(SbStatus::Ok)
    })
}

/// Removes the caller's markers: the file's markers are the TOC's again.
///
/// # Safety
///
/// `options` must be NULL or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_output_options_clear_markers(options: *const SbOutputOptions) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let h = unsafe { handle(options, "output options") }?;
        h.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?.dsdiff_markers = Markers::FromToc;
        Ok(SbStatus::Ok)
    })
}

/// Sets the tag items of the file to a copy of `tags` (W3): a DSF file's ID3 tag and
/// the DSDIFF artist and title. NULL tags from the TOC again.
///
/// # Safety
///
/// `options` must be NULL or a live handle; `tags` NULL or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_output_options_set_tags(
    options: *const SbOutputOptions,
    tags: *const SbTags,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let h = unsafe { handle(options, "output options") }?;
        let value = if tags.is_null() {
            Tags::FromToc
        } else {
            // SAFETY: forwarded contract.
            let t = unsafe { handle(tags, "tags") }?;
            Tags::Set(t.inner.try_borrow().map_err(|_| Failure::arg(REENTRANT))?.clone())
        };
        h.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?.tags = value;
        Ok(SbStatus::Ok)
    })
}

/// Sets a DSF file's tag to a copy of the `len` bytes of `tag`, written as it is (W3,
/// specification 8, 8.6 row 3). It is either an ID3v2.4 tag, checked for its header and
/// size, or a caller-supplied ID3v2.3 tag (8.2.6 item 4), checked only for its header:
/// `len >= 10`, "ID3", version 03 00, the low five flag bits 0, and four size bytes each
/// below 0x80 whose 28-bit size is `len - 10`; its frames are not checked. The check runs
/// when a layout is made, which fails with `SB_STATUS_INVALID_ARGUMENT` for any other
/// tag. A `len` above 1 MiB (1 048 576 bytes) is refused here.
///
/// # Safety
///
/// `options` must be NULL or a live handle; `tag` NULL or valid for `len` reads.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_output_options_set_raw_tag(
    options: *const SbOutputOptions,
    tag: *const u8,
    len: usize,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let h = unsafe { handle(options, "output options") }?;
        if len > scarletbook::output::MAX_TAG_BYTES {
            return Err(Failure::arg("the tag is larger than MAX_TAG_BYTES"));
        }
        // SAFETY: forwarded contract.
        let bytes = unsafe { slice(tag, len) }?;
        h.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?.tags = Tags::Raw(bytes.to_vec());
        Ok(SbStatus::Ok)
    })
}

/// Makes an empty set of tag items. On failure `*out` is set to NULL.
///
/// # Safety
///
/// `out` must be NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_tags_new(out: *mut *mut SbTags) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { clear_out(out) };
    guard(|| {
        non_null(out)?;
        // SAFETY: non-NULL and valid for a write by this function's contract.
        unsafe { give(out, SbTags { inner: RefCell::new(TagSet::new()) }) };
        Ok(SbStatus::Ok)
    })
}

/// The tag items the TOC gives a file of the area from `slot` with source track
/// `source_track` (0 for none), with the album-text and genre choices of `options` (NULL
/// for the defaults). On failure `*out` is set to NULL.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `options` NULL or a live handle; `out` NULL or
/// valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_tags_from_toc(
    disc: *const SbDisc,
    slot: u32,
    source_track: u32,
    options: *const SbOutputOptions,
    out: *mut *mut SbTags,
) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { clear_out(out) };
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?.get()?;
        non_null(out)?;
        let o = options_or_default(options)?;
        let area = d.toc().area(slot_from(slot)?).ok_or(Error::NoSuchArea(slot_from(slot)?))?;
        let track = match source_track {
            0 => None,
            n => Some(area.track(n).ok_or_else(|| Failure::no_such_track(area.slot, n))?),
        };
        let set = TagSet::from_toc(d.toc(), area, track, &o);
        // SAFETY: non-NULL and valid for a write by this function's contract.
        unsafe { give(out, SbTags { inner: RefCell::new(set) }) };
        Ok(SbStatus::Ok)
    })
}

/// A copy of the options of a handle, or the defaults for NULL.
fn options_or_default(options: *const SbOutputOptions) -> Result<OutputOptions, Failure> {
    if options.is_null() {
        return Ok(OutputOptions::default());
    }
    // SAFETY: NULL (handled above) or a live handle by the callers' contracts.
    let h = unsafe { handle(options, "output options") }?;
    Ok(h.inner.try_borrow().map_err(|_| Failure::arg(REENTRANT))?.clone())
}

/// Edits the tag items: `op` 0 sets `item` to `value` alone, 1 adds `value` as a further
/// value, 2 removes `item` (`value` is ignored).
///
/// # Safety
///
/// `tags` must be NULL or a live handle; `value` NULL or NUL-terminated.
unsafe fn edit(tags: *const SbTags, item: u32, value: *const c_char, op: u8) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let h = unsafe { handle(tags, "tags") }?;
        let item = tag_item(item)?;
        let mut t = h.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?;
        if op == 2 {
            t.remove(item);
            return Ok(SbStatus::Ok);
        }
        // SAFETY: forwarded contract.
        let v = unsafe { string(value) }?.ok_or_else(|| Failure::arg("NULL value"))?;
        if op == 0 {
            t.set(item, &v)
        } else {
            t.push(item, &v)
        }
        Ok(SbStatus::Ok)
    })
}

/// Sets `item` (`SbTagItem`) to the NUL-terminated UTF-8 `value` alone.
///
/// # Safety
///
/// `tags` must be NULL or a live handle; `value` NULL or NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_tags_set(tags: *const SbTags, item: u32, value: *const c_char) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { edit(tags, item, value, 0) }
}

/// Adds the NUL-terminated UTF-8 `value` as a further value of `item` (`SbTagItem`).
///
/// # Safety
///
/// `tags` must be NULL or a live handle; `value` NULL or NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_tags_push(tags: *const SbTags, item: u32, value: *const c_char) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { edit(tags, item, value, 1) }
}

/// Removes `item` (`SbTagItem`).
///
/// # Safety
///
/// `tags` must be NULL or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_tags_remove(tags: *const SbTags, item: u32) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { edit(tags, item, std::ptr::null(), 2) }
}

/// Sets the user-defined item `description` (an ID3 `TXXX` frame) to `value`; both are
/// NUL-terminated UTF-8.
///
/// # Safety
///
/// `tags` must be NULL or a live handle; `description` and `value` NULL or NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_tags_set_custom(
    tags: *const SbTags,
    description: *const c_char,
    value: *const c_char,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let h = unsafe { handle(tags, "tags") }?;
        // SAFETY: forwarded contract.
        let d = unsafe { string(description) }?.ok_or_else(|| Failure::arg("NULL description"))?;
        // SAFETY: forwarded contract.
        let v = unsafe { string(value) }?.ok_or_else(|| Failure::arg("NULL value"))?;
        h.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?.set_custom(&d, &v);
        Ok(SbStatus::Ok)
    })
}

/// The number of values of `item` (`SbTagItem`), 0 if it is absent.
///
/// # Safety
///
/// `tags` must be NULL or a live handle; `count` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_tags_count(tags: *const SbTags, item: u32, count: *mut u32) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let h = unsafe { handle(tags, "tags") }?;
        let n = h.inner.try_borrow().map_err(|_| Failure::arg(REENTRANT))?.get(tag_item(item)?).len();
        // SAFETY: forwarded contract.
        unsafe { write_out(count, u32::try_from(n).unwrap_or(u32::MAX)) }?;
        Ok(SbStatus::Ok)
    })
}

/// Copies value `index` of `item` (`SbTagItem`) out as a string (see the conventions):
/// `SB_STATUS_ABSENT` if there is no such value.
///
/// # Safety
///
/// `tags` must be NULL or a live handle; `buf` NULL or valid for `cap` bytes of writes;
/// `len` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_tags_get(
    tags: *const SbTags,
    item: u32,
    index: u32,
    buf: *mut c_char,
    cap: usize,
    len: *mut usize,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let h = unsafe { handle(tags, "tags") }?;
        let t = h.inner.try_borrow().map_err(|_| Failure::arg(REENTRANT))?;
        let v = t.get(tag_item(item)?).get(index as usize).map(String::as_str);
        // SAFETY: forwarded contract.
        unsafe { copy_string(v, buf, cap, len) }
    })
}

/// Frees tag items. NULL is ignored.
///
/// # Safety
///
/// `tags` must be NULL or a live handle, freed only once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_tags_free(tags: *mut SbTags) {
    // SAFETY: forwarded contract.
    unsafe { free(tags, |h| h.inner.try_borrow_mut().is_err()) };
}

/// Reads the lengths of the DST equivalents of the frame range `[first, end)` of the area
/// from `slot` (specification 8, 8.4.6 "Sizes"): without decoding, unless
/// `replace_invalid` is 1, which decodes every DST frame and gives invalid ones the length
/// of the 0x55-fill equivalent (W18). The disc's read and log callbacks run during this
/// call. Fails as `sb_range_check` does; on failure `*out` is set to NULL.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `out` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_dst_lengths_scan(
    disc: *const SbDisc,
    slot: u32,
    first: u32,
    end: u32,
    replace_invalid: u32,
    out: *mut *mut SbDstLengths,
) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { clear_out(out) };
    guard(|| {
        // SAFETY: forwarded contract.
        let mut d = unsafe { disc_handle(disc) }?.get_mut()?;
        non_null(out)?;
        let invalid =
            if flag(i64::from(replace_invalid))? { InvalidDst::Replaced } else { InvalidDst::Stored };
        let lengths = DstLengths::scan(&mut d, slot_from(slot)?, first, end, invalid)?;
        // SAFETY: non-NULL and valid for a write by this function's contract.
        unsafe { give(out, SbDstLengths { inner: RefCell::new(lengths) }) };
        Ok(SbStatus::Ok)
    })
}

/// Lengths saved from `sb_dst_lengths_get` of the frame range `[first, end)` of the area
/// from `slot`, `count` of them, scanned with `replace_invalid` as given: checked for
/// their count and bounds (1 to 65 536), not against the image. On failure `*out` is set
/// to NULL.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `lengths` NULL or valid for `count` reads; `out`
/// NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_dst_lengths_load(
    disc: *const SbDisc,
    slot: u32,
    first: u32,
    end: u32,
    replace_invalid: u32,
    lengths: *const u32,
    count: usize,
    out: *mut *mut SbDstLengths,
) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { clear_out(out) };
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?.get()?;
        non_null(out)?;
        // SAFETY: forwarded contract.
        let lengths = unsafe { slice(lengths, count) }?;
        let area = d.toc().area(slot_from(slot)?).ok_or(Error::NoSuchArea(slot_from(slot)?))?;
        let invalid =
            if flag(i64::from(replace_invalid))? { InvalidDst::Replaced } else { InvalidDst::Stored };
        let loaded = DstLengths::from_lengths(area, first, end, invalid, lengths.to_vec())?;
        // SAFETY: non-NULL and valid for a write by this function's contract.
        unsafe { give(out, SbDstLengths { inner: RefCell::new(loaded) }) };
        Ok(SbStatus::Ok)
    })
}

/// Copies the lengths out: `*count` receives their number; with `cap` at least that,
/// they are written to `out`, else `SB_STATUS_BUFFER_TOO_SMALL`.
///
/// # Safety
///
/// `lengths` must be NULL or a live handle; `out` NULL or valid for `cap` writes; `count`
/// NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_dst_lengths_get(
    lengths: *const SbDstLengths,
    out: *mut u32,
    cap: usize,
    count: *mut usize,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let h = unsafe { handle(lengths, "DST lengths") }?;
        let held = h.inner.try_borrow().map_err(|_| Failure::arg(REENTRANT))?;
        let l = held.lengths();
        // SAFETY: forwarded contract.
        unsafe { write_out(count, l.len()) }?;
        if out.is_null() || cap < l.len() {
            return Ok(SbStatus::BufferTooSmall);
        }
        // SAFETY: out is valid for cap >= l.len() writes, and cannot overlap our own vector.
        unsafe { std::ptr::copy_nonoverlapping(l.as_ptr(), out, l.len()) };
        Ok(SbStatus::Ok)
    })
}

/// Frees DST lengths. NULL is ignored, and so are lengths in use by a running call (the
/// call is then a no-op).
///
/// # Safety
///
/// `lengths` must be NULL or a live handle, freed only once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_dst_lengths_free(lengths: *mut SbDstLengths) {
    // SAFETY: forwarded contract.
    unsafe { free(lengths, |h| h.inner.try_borrow_mut().is_err()) };
}

/// The layout of the frame range `[first, end)` of the area from `slot` as a `format`
/// file (`SbOutputFormat`, DSF or DSDIFF with plain DSD), with `options` (NULL for the
/// defaults): its exact size and every byte that isn't audio, from the TOC alone. Fails as
/// `sb_range_check` does, with `SB_STATUS_OUTPUT_LAYOUT_UNSPECIFIED` or
/// `SB_STATUS_NOT_EDITED_MASTER`, or `SB_STATUS_INVALID_ARGUMENT` for an option the area
/// can't take; on failure `*out` is set to NULL.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `options` NULL or a live handle; `out` NULL or
/// valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_output_layout_new(
    disc: *const SbDisc,
    slot: u32,
    first: u32,
    end: u32,
    format: u32,
    options: *const SbOutputOptions,
    out: *mut *mut SbOutputLayout,
) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { clear_out(out) };
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?.get()?;
        non_null(out)?;
        let format = match format {
            0 => Format::Dsf,
            1 => Format::DsdiffDsd,
            2 => return Err(Failure::arg("a DST layout needs its lengths: sb_output_layout_new_dst")),
            _ => return Err(Failure::arg("unknown output format")),
        };
        let o = options_or_default(options)?;
        let layout = d.output_layout(slot_from(slot)?, first, end, format, &o)?;
        // SAFETY: non-NULL and valid for a write by this function's contract.
        unsafe { give(out, SbOutputLayout { inner: RefCell::new(layout) }) };
        Ok(SbStatus::Ok)
    })
}

/// The layout of a DSDIFF file with DST sound data holding the range of `lengths`
/// (specification 8, 8.4.6), with `options` (NULL for the defaults). Fails as
/// `sb_output_layout_new`; on failure `*out` is set to NULL.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `lengths` NULL or a live handle; `options` NULL
/// or a live handle; `out` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_output_layout_new_dst(
    disc: *const SbDisc,
    lengths: *const SbDstLengths,
    options: *const SbOutputOptions,
    out: *mut *mut SbOutputLayout,
) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { clear_out(out) };
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?.get()?;
        // SAFETY: forwarded contract.
        let l = unsafe { handle(lengths, "DST lengths") }?;
        non_null(out)?;
        let o = options_or_default(options)?;
        let held = l.inner.try_borrow().map_err(|_| Failure::arg(REENTRANT))?;
        let layout = d.output_layout_dst(&held, &o)?;
        // SAFETY: non-NULL and valid for a write by this function's contract.
        unsafe { give(out, SbOutputLayout { inner: RefCell::new(layout) }) };
        Ok(SbStatus::Ok)
    })
}

/// Describes a layout.
///
/// # Safety
///
/// `layout` must be NULL or a live handle; `info` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_output_layout_info(
    layout: *const SbOutputLayout,
    info: *mut SbOutputLayoutInfo,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let l = unsafe { handle(layout, "output layout") }?
            .inner
            .try_borrow()
            .map_err(|_| Failure::arg(REENTRANT))?;
        let (first, end) = l.range();
        let value = SbOutputLayoutInfo {
            size: l.size(),
            format: match l.format() {
                Format::Dsf => 0,
                Format::DsdiffDsd => 1,
                _ => 2,
            },
            slot: slot_index(l.slot()),
            first,
            end,
            channels: u32::from(l.channels()),
            source_track: l.source_track().map_or(0, u32::from),
            input: match l.input() {
                Input::Content => 0,
                Input::DstEquivalents => 1,
                _ => 2,
            },
            regions: u32::try_from(l.regions().len()).unwrap_or(u32::MAX),
            memory: l.memory() as u64,
        };
        // SAFETY: forwarded contract.
        unsafe { write_out(info, value) }?;
        Ok(SbStatus::Ok)
    })
}

/// Region `index` of a layout, in file order: `SB_STATUS_INVALID_ARGUMENT` past the last.
///
/// # Safety
///
/// `layout` must be NULL or a live handle; `region` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_output_layout_region(
    layout: *const SbOutputLayout,
    index: u32,
    region: *mut SbOutputRegion,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let l = unsafe { handle(layout, "output layout") }?
            .inner
            .try_borrow()
            .map_err(|_| Failure::arg(REENTRANT))?;
        let r = l.regions().get(index as usize).ok_or_else(|| Failure::arg("no such region"))?;
        let kind = match r.kind {
            RegionKind::Header => 0,
            RegionKind::SoundData => 1,
            _ => 2,
        };
        // SAFETY: forwarded contract.
        unsafe { write_out(region, SbOutputRegion { offset: r.offset, len: r.len, kind, reserved: 0 }) }?;
        Ok(SbStatus::Ok)
    })
}

/// The timeline positions `[*first, *end)` that producing the file's bytes
/// `[offset, offset + len)` may read; empty (`*first == *end`) when they need none.
///
/// # Safety
///
/// `layout` must be NULL or a live handle; `first` and `end` NULL or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_output_layout_positions(
    layout: *const SbOutputLayout,
    offset: u64,
    len: u64,
    first: *mut u32,
    end: *mut u32,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let l = unsafe { handle(layout, "output layout") }?
            .inner
            .try_borrow()
            .map_err(|_| Failure::arg(REENTRANT))?;
        let p = l.positions_for(offset..offset.saturating_add(len));
        // SAFETY: forwarded contract.
        unsafe { write_out(first, p.start) }?;
        // SAFETY: forwarded contract.
        unsafe { write_out(end, p.end) }?;
        Ok(SbStatus::Ok)
    })
}

/// Reads the file's bytes from `offset` into `buf` (`len` bytes), taking the positions
/// from `disc`, which must be of the image the layout was made from
/// (`SB_STATUS_INVALID_ARGUMENT` otherwise); `*written` receives the bytes written, fewer
/// than `len` at the end of the file. The disc's read and log callbacks run during this
/// call.
///
/// # Safety
///
/// `layout` and `disc` must be NULL or live handles; `buf` NULL or valid for `len` bytes
/// of writes; `written` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_output_layout_read(
    layout: *const SbOutputLayout,
    disc: *const SbDisc,
    offset: u64,
    buf: *mut u8,
    len: usize,
    written: *mut usize,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let l = unsafe { handle(layout, "output layout") }?
            .inner
            .try_borrow()
            .map_err(|_| Failure::arg(REENTRANT))?;
        // SAFETY: forwarded contract.
        let mut d = unsafe { disc_handle(disc) }?.get_mut()?;
        if written.is_null() {
            return Err(Failure::arg("NULL output pointer"));
        }
        if buf.is_null() && len > 0 {
            return Err(Failure::arg("NULL buffer"));
        }
        let out: &mut [u8] = if len == 0 {
            &mut []
        } else {
            // SAFETY: non-NULL and valid for len writes by this function's contract.
            unsafe { std::slice::from_raw_parts_mut(buf, len) }
        };
        let n = l.read_at(&mut d, offset, out)?;
        // SAFETY: forwarded contract.
        unsafe { write_out(written, n) }?;
        Ok(SbStatus::Ok)
    })
}

/// Positions the caller holds.
struct Held<'a> {
    positions: &'a [SbPositionData],
}

impl Held<'_> {
    fn find(&self, t: u32) -> Result<&SbPositionData, Error> {
        self.positions
            .iter()
            .find(|p| p.number == t)
            .ok_or(Error::InvalidArgument("a needed position was not given"))
    }
}

impl PositionSource for Held<'_> {
    fn content(&mut self, t: u32) -> Result<Cow<'_, [u8]>, Error> {
        let p = self.find(t)?;
        if p.content.is_null() {
            return Err(Error::InvalidArgument("a needed position's content was not given"));
        }
        // SAFETY: valid for content_len reads by sb_output_layout_fill's contract.
        Ok(Cow::Borrowed(unsafe { std::slice::from_raw_parts(p.content, p.content_len) }))
    }

    fn dst_equivalent(&mut self, t: u32) -> Result<Cow<'_, [u8]>, Error> {
        let p = self.find(t)?;
        if p.dst_equivalent.is_null() {
            return Err(Error::InvalidArgument("a needed position's DST equivalent was not given"));
        }
        // SAFETY: valid for dst_equivalent_len reads by sb_output_layout_fill's contract.
        Ok(Cow::Borrowed(unsafe { std::slice::from_raw_parts(p.dst_equivalent, p.dst_equivalent_len) }))
    }
}

/// Fills `buf` (`len` bytes) with the file's bytes from `offset`, from the `count`
/// positions the caller gives: those of `sb_output_layout_positions` for the same bytes,
/// with what `SbOutputLayoutInfo.input` says they need. `*written` receives the bytes
/// written, fewer than `len` at the end of the file. `SB_STATUS_INVALID_ARGUMENT` if a
/// needed position, or its content or DST equivalent, is missing or of the wrong size, or
/// if `buf` overlaps the `positions` array or any position's content or DST equivalent.
///
/// # Safety
///
/// `layout` must be NULL or a live handle; `buf` NULL or valid for `len` bytes of writes;
/// `positions` NULL or valid for `count` reads, each with its non-NULL pointers valid for
/// their lengths; `written` NULL or valid for a write. `buf` must not overlap `positions`
/// or the bytes they point to (checked).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_output_layout_fill(
    layout: *const SbOutputLayout,
    offset: u64,
    buf: *mut u8,
    len: usize,
    positions: *const SbPositionData,
    count: usize,
    written: *mut usize,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let l = unsafe { handle(layout, "output layout") }?
            .inner
            .try_borrow()
            .map_err(|_| Failure::arg(REENTRANT))?;
        // SAFETY: forwarded contract.
        let positions = unsafe { slice(positions, count) }?;
        if written.is_null() {
            return Err(Failure::arg("NULL output pointer"));
        }
        if buf.is_null() && len > 0 {
            return Err(Failure::arg("NULL buffer"));
        }
        // `buf` becomes a `&mut [u8]` while the positions' bytes are borrowed: refuse any
        // overlap, which address comparison finds without reading through the pointers.
        let overlaps = |p: *const u8, n: usize| {
            let (a, b) = (p.addr(), buf.addr());
            n > 0 && len > 0 && a < b.saturating_add(len) && b < a.saturating_add(n)
        };
        let bytes = |p: &SbPositionData| {
            [(p.content, p.content_len), (p.dst_equivalent, p.dst_equivalent_len)]
                .into_iter()
                .any(|(q, n)| !q.is_null() && overlaps(q, n))
        };
        if overlaps(positions.as_ptr().cast(), size_of_val(positions)) || positions.iter().any(bytes) {
            return Err(Failure::arg("the buffer overlaps the positions or their bytes"));
        }
        let out: &mut [u8] = if len == 0 {
            &mut []
        } else {
            // SAFETY: non-NULL and valid for len writes by this function's contract, and
            // overlapping none of the positions' bytes (checked above).
            unsafe { std::slice::from_raw_parts_mut(buf, len) }
        };
        let n = l.fill(offset, out, &mut Held { positions })?;
        // SAFETY: forwarded contract.
        unsafe { write_out(written, n) }?;
        Ok(SbStatus::Ok)
    })
}

/// Frees a layout. NULL is ignored, and so is a layout in use by a running call, such as a
/// read whose callback frees it (the call is then a no-op).
///
/// # Safety
///
/// `layout` must be NULL or a live handle, freed only once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_output_layout_free(layout: *mut SbOutputLayout) {
    // SAFETY: forwarded contract.
    unsafe { free(layout, |h| h.inner.try_borrow_mut().is_err()) };
}

/// Adds a DSDIFF caller chunk (specification 8, 8.4.2 item 5, at sacd-spec `e069569`; 8.6 row
/// 33): the 4 bytes of `id`, and a copy of the `len` bytes of `data` as its data. Caller
/// chunks are written in the order added, after the last local chunk, with a pad byte 0x00
/// when `len` is odd (not counted in the chunk's size); they count in the `FRM8` size and
/// in the layout's last region (`SB_OUTPUT_REGION_KIND_TRAILER`). The same ID may be added more
/// than once, and an edited master may have them. A DSF file ignores them. Their data is the
/// caller's: neither the DSDIFF document nor specification 8 defines it.
///
/// `SB_STATUS_INVALID_ARGUMENT` if `id` is not 4 bytes 0x20 to 0x7E with no space before a
/// non-space (8.4.1 item 3), or is one of `FRM8`, `FVER`, `PROP`, `DSD `, `DST `, `DSTI`,
/// `COMT`, `DIIN` and `MANF`; the options are then unchanged.
///
/// # Safety
///
/// `options` must be NULL or a live handle; `id` NULL or valid for 4 reads; `data` NULL or
/// valid for `len` reads.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_output_options_add_chunk(
    options: *const SbOutputOptions,
    id: *const u8,
    data: *const u8,
    len: usize,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let h = unsafe { handle(options, "output options") }?;
        if id.is_null() {
            return Err(Failure::arg("NULL chunk ID"));
        }
        // SAFETY: forwarded contract; non-NULL, so valid for 4 reads.
        let id = unsafe { slice(id, 4) }?;
        let id: [u8; 4] = id.try_into().map_err(|_| Failure::arg("a chunk ID is 4 bytes"))?;
        // SAFETY: forwarded contract.
        let data = unsafe { slice(data, len) }?;
        let chunk = CallerChunk::new(id, data.to_vec())?;
        h.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?.dsdiff_caller_chunks.push(chunk);
        Ok(SbStatus::Ok)
    })
}

/// Removes every DSDIFF caller chunk.
///
/// # Safety
///
/// `options` must be NULL or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_output_options_clear_chunks(options: *const SbOutputOptions) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let h = unsafe { handle(options, "output options") }?;
        h.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?.dsdiff_caller_chunks.clear();
        Ok(SbStatus::Ok)
    })
}

/// One file of a chain of ranges (`sb_output_dsf_chain_plan`; specification 8, 8.2.7,
/// sacd-spec e069569).
///
/// Layout: 40 bytes, aligned to 8, without padding (2 fields of 8 bytes, then 6 of 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SbDsfChainFile {
    /// `p_i`, the first byte of each channel's byte timeline the file holds (8.2.7 items 1
    /// to 3); 0 when `status` is not `SB_STATUS_OK`.
    pub byte_first: u64,
    /// `p_{i+1}`, the end of those bytes: the stream holds `S = byte_end - byte_first`
    /// bytes per channel.
    pub byte_end: u64,
    /// `a_i`, the range's first frame.
    pub first: u32,
    /// `b_i`, the range's end frame (exclusive).
    pub end: u32,
    /// `a_1` of the file's chain, for the option `SB_OUTPUT_OPTION_DSF_CHAIN_FIRST`; 0 when `status` is not
    /// `SB_STATUS_OK`.
    pub chain_first: u32,
    /// `b_K` of the file's chain, for the option `SB_OUTPUT_OPTION_DSF_CHAIN_END`; 0 when `status` is not
    /// `SB_STATUS_OK`.
    pub chain_end: u32,
    /// `SB_STATUS_OK`, or the status of the checks that skip or refuse the range (8.1 item
    /// 2): the one `sb_output_layout_new` gives for it, such as `SB_STATUS_TRACK_EMPTY`.
    /// Such a range splits the chain (8.2.7 item 3).
    pub status: i32,
    /// 0.
    pub reserved: u32,
}

/// Plans the DSF files of a chain of ranges of the area from `slot` (specification 8,
/// 8.2.7 and 8.6 row 32, sacd-spec e069569), from the TOC alone: `bounds` are the `count`
/// frames `a_1, a_2, ..., a_K, b_K`, so range `i` is `[bounds[i - 1], bounds[i])`, for
/// example the tracks with pause (`sb_track_range_with_pause`) of consecutive tracks
/// that meet end to end. `files` receives one `SbDsfChainFile` per range, `count - 1` of
/// them, and `*written` their number; with `cap` below that, nothing is written to
/// `files` and the call gives `SB_STATUS_BUFFER_TOO_SMALL`.
///
/// Every range is checked by 8.1 item 2 before any entry is written (8.2.7 item 3 at
/// sacd-spec fa558a8). A range that the checks skip or refuse has that status in its entry
/// and splits the chain, an empty range (`SB_STATUS_TRACK_EMPTY`) included even though its
/// neighbours meet: the ranges before it and after it form separate chains, each with its
/// own `chain_first` and `chain_end`. Each other range's file is
/// `sb_output_layout_new(disc, slot, first, end, SB_OUTPUT_FORMAT_DSF, options, ...)`
/// with the options `SB_OUTPUT_OPTION_DSF_CHAIN_FIRST` and `SB_OUTPUT_OPTION_DSF_CHAIN_END` set to its `chain_first` and
/// `chain_end`. Which files are written is the caller's choice (8.2.7 item 7): one not
/// written does not split the chain. Another chain is another plan.
///
/// `SB_STATUS_NO_SUCH_AREA`; `SB_STATUS_INVALID_ARGUMENT` for fewer than two bounds.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `bounds` NULL or valid for `count` reads; `files`
/// NULL or valid for `cap` writes; `written` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_output_dsf_chain_plan(
    disc: *const SbDisc,
    slot: u32,
    bounds: *const u32,
    count: usize,
    files: *mut SbDsfChainFile,
    cap: usize,
    written: *mut usize,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?.get()?;
        // SAFETY: forwarded contract.
        let bounds = unsafe { slice(bounds, count) }?;
        let plan = plan_dsf_chain(d.toc(), slot_from(slot)?, bounds)?;
        // SAFETY: forwarded contract.
        unsafe { write_out(written, plan.len()) }?;
        if files.is_null() || cap < plan.len() {
            return Ok(SbStatus::BufferTooSmall);
        }
        for (i, (file, w)) in plan.into_iter().zip(bounds.windows(2)).enumerate() {
            let value = match file {
                Ok(f) => SbDsfChainFile {
                    byte_first: f.bytes.0,
                    byte_end: f.bytes.1,
                    first: f.range.0,
                    end: f.range.1,
                    chain_first: f.chain.0,
                    chain_end: f.chain.1,
                    status: SbStatus::Ok as i32,
                    reserved: 0,
                },
                Err(e) => SbDsfChainFile {
                    first: w[0],
                    end: w[1],
                    status: Failure::from(e).0 as i32,
                    ..SbDsfChainFile::default()
                },
            };
            // SAFETY: i < plan.len() <= cap, within the array of this function's contract.
            let at = unsafe { files.add(i) };
            // SAFETY: non-NULL and valid for a write by this function's contract; written
            // without reading what was there, which may be uninitialized.
            unsafe { at.write(value) };
        }
        Ok(SbStatus::Ok)
    })
}
