// SPDX-License-Identifier: MIT OR Apache-2.0

//! C ABI over the `scarletbook` crate.
//!
//! The conventions every function follows (status codes, handles, copy-out strings,
//! threading, callbacks and re-entrancy, panics) are stated once, at the top of the
//! generated header `include/scarletbook.h`; their source is the `header` of
//! `cbindgen.toml`.

use std::cell::{Ref, RefCell, RefMut};
use std::ffi::{CStr, CString, c_char, c_void};
use std::fs::File;
#[cfg(not(any(unix, windows)))]
use std::io::BufReader;
use std::io::{self, Read, Seek, SeekFrom};
use std::mem::offset_of;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use scarletbook::log::{Level, Log};
use scarletbook::toc::{AreaTextItem, DiscTextItem, TrackTextItem};
use scarletbook::{
    Area, AreaCursor, AreaIndex, AudioFrame, CancelToken, DecodeOn, Disc, Error, FrameContent, FrameKind,
    FrameStatus, IndexOptions, LoudspeakerLayout, MasterToc, Progress, ReadAt, ReadAtReader, SeekPolicy,
    Slot, Speaker, Speculation, SpeculativeOptions, Stored, Track, TrackAudioCursor, TrackCursor, TrackFrame,
    WarningCode,
};

/// Result of every call: an `int32_t`, whose values are the `SB_STATUS_*` constants. In C
/// before C23, `SbStatus` is a typedef of `int32_t`; in C23 and C++, an enumeration whose
/// underlying type is `int32_t`.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SbStatus {
    /// Success.
    Ok = 0,
    /// A track's audio has no more frames.
    End = 1,
    /// The requested string is absent.
    Absent = 2,
    /// Rejected: `not-scarletbook`.
    NotScarletbook = -1,
    /// Rejected: `master-toc-truncated`.
    MasterTocTruncated = -2,
    /// Rejected: `master-toc-version-unsupported`.
    MasterTocVersionUnsupported = -3,
    /// Rejected: `no-audio-areas`.
    NoAudioAreas = -4,
    /// Track refused: `channel-count-unsupported`.
    ChannelCountUnsupported = -5,
    /// Track skipped: `track-empty`.
    TrackEmpty = -6,
    /// Track refused: `track-exceeds-capacity`.
    TrackExceedsCapacity = -7,
    /// No area from that slot.
    NoSuchArea = -8,
    /// No track with that number.
    NoSuchTrack = -9,
    /// A NULL or otherwise invalid argument, or a handle used re-entrantly from a callback.
    InvalidArgument = -10,
    /// The output buffer is too small.
    BufferTooSmall = -11,
    /// The reader failed.
    Io = -12,
    /// The DST frame is invalid (`sb_dst_decode`).
    InvalidDstFrame = -13,
    /// An internal error (a bug in this library). Free the handles involved.
    Panic = -14,
    /// A frame range is reversed or extends past the area's play time (a caller error).
    FrameRangeInvalid = -15,
    /// The operating system could not start a decoding pool's threads (`sb_pool_new`).
    ThreadsUnavailable = -16,
    /// Memory for a copy the caller asked for could not be allocated
    /// (`sb_disc_open_memory`).
    OutOfMemory = -17,
    /// A sector range extends past the image's last sector (`sb_disc_read_sectors`).
    SectorRangeInvalid = -18,
    /// The call stopped because cancellation was requested (`sb_cancel_request`); the
    /// handle stays where it was and continues once the request is withdrawn.
    Cancelled = -19,
}

/// Area slots, as passed in `slot` arguments.
#[repr(u32)]
#[derive(Clone, Copy, Debug)]
pub enum SbSlot {
    /// The 2-channel slot.
    TwoChannel = 0,
    /// The multichannel slot.
    Multichannel = 1,
}

/// How a decoded frame was produced (`SbFrameInfo.status`).
#[repr(u32)]
#[derive(Clone, Copy, Debug)]
pub enum SbFrameStatus {
    /// A plain DSD frame, passed through.
    Dsd = 0,
    /// A DST frame, decoded.
    Decoded = 1,
    /// Missing: filled with 0x69.
    Missing = 2,
    /// An invalid DST frame: filled with 0x55.
    Invalid = 3,
}

/// What a timeline position holds, undecoded (`SbRawFrameInfo.kind`).
#[repr(u32)]
#[derive(Clone, Copy, Debug)]
pub enum SbFrameKind {
    /// A plain DSD frame: channels x 4704 bytes, interleaved.
    Dsd = 0,
    /// A DST-coded frame: its bytes as stored (the DST frame, 1 to 65 536 bytes).
    Dst = 1,
    /// No frame fills the position: no bytes (decoded reads fill it with 0x69).
    Missing = 2,
}

/// Disc-level strings for `sb_disc_string`: the 16 disc text items of specification 2,
/// 2.5.4, then the two catalogue numbers.
#[repr(u32)]
#[derive(Clone, Copy, Debug)]
pub enum SbDiscString {
    /// Album title.
    AlbumTitle = 0,
    /// Album artist.
    AlbumArtist = 1,
    /// Album publisher.
    AlbumPublisher = 2,
    /// Album copyright.
    AlbumCopyright = 3,
    /// Album title, phonetic.
    AlbumTitlePhonetic = 4,
    /// Album artist, phonetic.
    AlbumArtistPhonetic = 5,
    /// Album publisher, phonetic.
    AlbumPublisherPhonetic = 6,
    /// Album copyright, phonetic.
    AlbumCopyrightPhonetic = 7,
    /// Disc title.
    DiscTitle = 8,
    /// Disc artist.
    DiscArtist = 9,
    /// Disc publisher.
    DiscPublisher = 10,
    /// Disc copyright.
    DiscCopyright = 11,
    /// Disc title, phonetic.
    DiscTitlePhonetic = 12,
    /// Disc artist, phonetic.
    DiscArtistPhonetic = 13,
    /// Disc publisher, phonetic.
    DiscPublisherPhonetic = 14,
    /// Disc copyright, phonetic.
    DiscCopyrightPhonetic = 15,
    /// Album catalogue number (always present; may be empty).
    AlbumCatalogNumber = 16,
    /// Disc catalogue number (always present; may be empty).
    DiscCatalogNumber = 17,
}

/// Area strings for `sb_area_string`, in the order of specification 3, 3.3.
#[repr(u32)]
#[derive(Clone, Copy, Debug)]
pub enum SbAreaString {
    /// Description.
    Description = 0,
    /// Copyright.
    Copyright = 1,
    /// Description, phonetic.
    DescriptionPhonetic = 2,
    /// Copyright, phonetic.
    CopyrightPhonetic = 3,
}

/// Track strings for `sb_track_string`: the text item types 0x01 to 0x07 and 0x81 to 0x87
/// of specification 3, 3.8, then the ISRC.
#[repr(u32)]
#[derive(Clone, Copy, Debug)]
pub enum SbTrackString {
    /// Title (type 0x01).
    Title = 0,
    /// Performer (type 0x02).
    Performer = 1,
    /// Songwriter (type 0x03).
    Songwriter = 2,
    /// Composer (type 0x04).
    Composer = 3,
    /// Arranger (type 0x05).
    Arranger = 4,
    /// Message (type 0x06).
    Message = 5,
    /// Extra message (type 0x07).
    ExtraMessage = 6,
    /// Title, phonetic (type 0x81).
    TitlePhonetic = 7,
    /// Performer, phonetic (type 0x82).
    PerformerPhonetic = 8,
    /// Songwriter, phonetic (type 0x83).
    SongwriterPhonetic = 9,
    /// Composer, phonetic (type 0x84).
    ComposerPhonetic = 10,
    /// Arranger, phonetic (type 0x85).
    ArrangerPhonetic = 11,
    /// Message, phonetic (type 0x86).
    MessagePhonetic = 12,
    /// Extra message, phonetic (type 0x87).
    ExtraMessagePhonetic = 13,
    /// ISRC.
    Isrc = 14,
}

/// Disc-level information.
///
/// Layout: 32 bytes, without padding: `sectors` at offset 0, `sector_size` at 8,
/// `version_major` and `version_minor` at 12 and 13, `album_set_size` at 14,
/// `album_sequence_number` at 16, `hybrid` and `manufacturer_sector` at 18 and 19, `year`
/// at 20, `month` and `day` at 22 and 23, `area_count` at 24, `warning_count` at 28.
/// Aligned to 8 (to 4 where `uint64_t` is, as on 32-bit x86).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SbDiscInfo {
    /// Number of logical sectors in the image.
    pub sectors: u64,
    /// Stored sector size: 2048, 2054 or 2064.
    pub sector_size: u32,
    /// Master TOC version.
    pub version_major: u8,
    /// Master TOC version.
    pub version_minor: u8,
    /// Album set size.
    pub album_set_size: u16,
    /// Album sequence number.
    pub album_sequence_number: u16,
    /// Hybrid disc flag: 1 or 0.
    pub hybrid: u8,
    /// Whether the manufacturer sector is present: 1 or 0.
    pub manufacturer_sector: u8,
    /// Date (0 = not set).
    pub year: u16,
    /// Date (0 = not set).
    pub month: u8,
    /// Date (0 = not set).
    pub day: u8,
    /// Number of areas (1 or 2).
    pub area_count: u32,
    /// Number of warnings.
    pub warning_count: u32,
}

/// Information about one area.
///
/// Layout: 52 bytes, aligned to 4, without padding (13 fields of 4 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SbAreaInfo {
    /// `SbSlot` it came from.
    pub slot: u32,
    /// 2 for a `TWOCHTOC` area, 6 for `MULCHTOC`.
    pub kind_channels: u32,
    /// LSN of the TOC copy used.
    pub toc_lsn: u32,
    /// Area TOC size in sectors.
    pub toc_sectors: u32,
    /// Channel count (audio only for 1 to 6).
    pub channel_count: u32,
    /// Area configuration byte.
    pub area_config: u32,
    /// Sample frequency code (4 = DSD64).
    pub sample_frequency_code: u32,
    /// Frame format (low 4 bits; informative).
    pub frame_format: u32,
    /// Maximum byte rate.
    pub max_byte_rate: u32,
    /// Play time in frames.
    pub playtime_frames: u32,
    /// First audio LSN.
    pub audio_start_lsn: u32,
    /// Last audio LSN (inclusive).
    pub audio_end_lsn: u32,
    /// Number of tracks, numbered from 1.
    pub track_count: u32,
}

/// Information about one track.
///
/// Layout: 28 bytes, aligned to 4, without padding: the five `uint32_t` at offsets 0 to
/// 16, the five 1-byte fields at 20 to 24, then `reserved` at 25 to 27.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SbTrackInfo {
    /// Track number.
    pub number: u32,
    /// Start LSN, as stored.
    pub start_lsn: u32,
    /// Length in sectors, as stored.
    pub length_sectors: u32,
    /// First frame (after clamping).
    pub start_frame: u32,
    /// Duration in frames (after clamping).
    pub duration_frames: u32,
    /// Start flags byte.
    pub start_flags: u8,
    /// Duration flags byte.
    pub duration_flags: u8,
    /// Whether the area has genres (`SACD_IGL`): 1 or 0.
    pub has_genre: u8,
    /// Genre table byte.
    pub genre_category: u8,
    /// Genre index byte.
    pub genre_index: u8,
    /// Reserved: written as 0.
    pub reserved: [u8; 3],
}

/// A warning.
///
/// Layout: `lsn` at offset 0, `slot` at 8, `track` at 12, `code` at 16. On 64-bit
/// targets 24 bytes, aligned to 8, without padding. On 32-bit targets the pointer `code`
/// takes 4 bytes: 20 bytes in all, and 4 bytes of padding at the end where `int64_t` is
/// aligned to 8 (as on 32-bit Arm).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SbWarning {
    /// The LSN the warning concerns (`audio-sector-malformed`), or -1.
    pub lsn: i64,
    /// `SbSlot`, or -1 if the warning concerns no area.
    pub slot: i32,
    /// Track number, or 0 if the warning concerns no track.
    pub track: u32,
    /// The diagnostic code of specification 7 (a static string).
    pub code: *const c_char,
}

/// Information about one decoded frame.
///
/// Layout: 16 bytes, aligned to 4, without padding (4 fields of 4 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SbFrameInfo {
    /// The frame number.
    pub number: u32,
    /// `SbFrameStatus`.
    pub status: u32,
    /// For an invalid DST frame, the rule of specification 5, 5.6; otherwise 0.
    pub dst_rule: u32,
    /// Bytes written: channels x 4704.
    pub size: u32,
}

/// The largest frame `sb_raw_frames_read` returns, in bytes (specification 4, 4.4.3): a
/// buffer of this size always suffices.
pub const SB_MAX_FRAME_BYTES: usize = 65_536;

// A literal, so that cbindgen can emit it; it must stay the core crate's bound.
const _: () = assert!(SB_MAX_FRAME_BYTES == scarletbook::audio::MAX_FRAME_BYTES);

/// Information about one undecoded timeline position.
///
/// Layout: 12 bytes, aligned to 4, without padding (3 fields of 4 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SbRawFrameInfo {
    /// The frame number.
    pub number: u32,
    /// `SbFrameKind`.
    pub kind: u32,
    /// The frame's size in bytes (0 for a missing position): the bytes written, or with
    /// `SB_STATUS_BUFFER_TOO_SMALL` the bytes needed. At most `SB_MAX_FRAME_BYTES`.
    pub size: u32,
}

/// A genre entry as stored (specification 2, 2.4): `sb_disc_genre`.
///
/// Layout: 2 bytes, aligned to 1, without padding.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SbGenre {
    /// Genre table (category) byte: 1 is the general table, 2 a Japanese one.
    pub category: u8,
    /// Genre index byte.
    pub index: u8,
}

/// A text channel's locale entry as stored (specification 2, 2.5.1): `sb_disc_text_channel`,
/// `sb_area_text_channel`.
///
/// Layout: 4 bytes, aligned to 1, without padding: `language` at 0, `charset` at 2,
/// `reserved` at 3.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SbTextChannel {
    /// The language code's two bytes as stored (an ISO 639-1 code such as "en"), nothing
    /// removed: 0x00 0x00 is an unset code. The trimmed language of specification 2, 2.5.1
    /// item 1 is these bytes with every trailing 0x00 and 0x20 removed.
    pub language: [u8; 2],
    /// Character set code, the stored byte (the low 3 bits select the character set).
    pub charset: u8,
    /// Written as 0.
    pub reserved: u8,
}

/// More of an area's TOC fields (specification 3, 3.4 and 3.5): `sb_area_details`.
///
/// Layout: 8 bytes, aligned to 1, without padding (8 fields of 1 byte).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SbAreaDetails {
    /// Area TOC version (offset 8).
    pub version_major: u8,
    /// Area TOC version (offset 9).
    pub version_minor: u8,
    /// Maximum available channels (offset 34), as stored.
    pub max_available_channels: u8,
    /// Area mute flags (offset 35), as stored.
    pub mute_flags: u8,
    /// Track attribute: the low 4 bits of offset 48.
    pub track_attribute: u8,
    /// Track offset (offset 68), as stored.
    pub track_offset: u8,
    /// Text channel count (offset 80), as stored; `sb_area_text_channel` reads the first
    /// `min(count, 10)` locale entries.
    pub text_channel_count: u8,
    /// Character set code of area locale entry 0 (offset 90), with which all area text is
    /// decoded, also when the text channel count is 0.
    pub text_charset: u8,
}

type Reader = Box<dyn ReadSeek + Send>;

/// An opened disc image.
///
/// Its state sits in a `RefCell`: every call takes a shared reference to the handle and
/// borrows the state, so a call made from inside one of this disc's callbacks finds the
/// state borrowed and fails cleanly instead of aliasing it.
pub struct SbDisc {
    inner: RefCell<Disc<Reader>>,
    /// The disc's number, shared by its clones (`sb_disc_clone`): they read the same image,
    /// so a track or raw frames handle of one may be read through any of them.
    id: u64,
    /// What a clone reads through.
    source: DiscSource,
    /// The C log callback the disc's log calls, if any; replaced only while `inner` can be
    /// borrowed mutably, and cleared when the disc is dropped.
    log: RefCell<Option<Arc<LogTarget>>>,
    /// Verdicts on provisional readings taken from the disc's queue and not yet returned
    /// by `sb_disc_seek_event_next`, per slot.
    events: RefCell<[std::collections::VecDeque<scarletbook::SeekEvent>; 2]>,
}

impl Drop for SbDisc {
    fn drop(&mut self) {
        // Decoding jobs still on the pool hold the disc's log: make sure they no longer
        // reach the callback once sb_disc_free returns.
        if let Some(target) = self.log.get_mut().take() {
            target.clear();
        }
    }
}

impl std::fmt::Debug for SbDisc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SbDisc").field("id", &self.id).finish_non_exhaustive()
    }
}

/// The decoding state of one track or frame range. Each `sb_track_audio_read` call reads through the
/// disc it was opened from or a clone of it; that disc must be live for those calls (it may be freed
/// before the track handle, which can then only be freed).
#[derive(Debug)]
pub struct SbTrackAudio {
    inner: RefCell<TrackAudioCursor>,
    disc_id: u64,
    /// A position whose stored content the caller's buffer was too small for
    /// (`sb_track_audio_read_with_stored`), returned by the next read.
    held: RefCell<Option<(AudioFrame, Stored)>>,
}

impl SbTrackAudio {
    /// A new handle of a disc's cursor that decodes where handles do by default: on the
    /// calling thread, or with the `parallel` feature on the current rayon pool.
    fn fresh(mut cursor: TrackAudioCursor, disc_id: u64) -> Box<Self> {
        cursor.set_decode_on(Self::default_decode_on());
        Box::new(SbTrackAudio { inner: RefCell::new(cursor), disc_id, held: RefCell::new(None) })
    }

    /// Where handles decode without a pool of their own.
    fn default_decode_on() -> DecodeOn {
        if cfg!(feature = "parallel") { DecodeOn::CurrentPool } else { DecodeOn::CallingThread }
    }
}

/// A pool of decoding threads (`sb_pool_new`), shared by the track audio handles given it.
/// Unlike the other handles it holds no state of its own beyond its threads, so it may be
/// used by several threads at once.
#[derive(Debug)]
pub struct SbPool {
    pool: Arc<rayon::ThreadPool>,
    threads: u32,
}

/// The reading state of one frame range's stored content, or of an area's stored frame
/// list, without decoding. Each `sb_raw_frames_read` call reads through the disc it was
/// opened from or a clone of it, as for `SbTrackAudio`.
#[derive(Debug)]
pub struct SbRawFrames {
    inner: RefCell<RawFrames>,
    disc_id: u64,
}

#[derive(Debug)]
struct RawFrames {
    source: Source,
    /// A frame the caller's buffer was too small for, returned by the next read.
    held: Option<TrackFrame>,
}

/// Where a raw frames handle's positions come from (specification 4, 4.6.1 item 2).
#[derive(Debug)]
enum Source {
    /// The stored content of a frame range: every position, missing ones included.
    Range(TrackCursor),
    /// The area's stored frame list, of an area of this many channels: every accepted
    /// frame, no missing positions.
    List(AreaCursor, u8),
}

impl RawFrames {
    fn channels(&self) -> u8 {
        match &self.source {
            Source::Range(c) => c.channels(),
            Source::List(_, channels) => *channels,
        }
    }

    fn next_frame(&mut self, image: &mut scarletbook::Image<Reader>) -> Option<Result<TrackFrame, Error>> {
        match &mut self.source {
            Source::Range(c) => c.next_frame(image),
            Source::List(c, _) => c.next_frame(image).map(|f| {
                f.map(|f| {
                    let content = match f.kind {
                        FrameKind::Dst => FrameContent::Dst(f.data),
                        FrameKind::Dsd => FrameContent::Dsd(f.data),
                    };
                    TrackFrame { number: f.number, content }
                })
            }),
        }
    }

    /// The DST equivalent of a position of a range (stored frame lists have none).
    fn dst_equivalent<'c>(&self, content: &'c FrameContent) -> Option<std::borrow::Cow<'c, [u8]>> {
        match &self.source {
            Source::Range(c) => Some(c.dst_equivalent(content)),
            Source::List(..) => None,
        }
    }

    fn missing_frames(&self) -> u64 {
        match &self.source {
            Source::Range(c) => c.missing_frames(),
            Source::List(..) => 0,
        }
    }

    fn malformed_sectors(&self) -> u64 {
        match &self.source {
            Source::Range(c) => c.malformed_sectors(),
            Source::List(c, _) => c.malformed_sectors(),
        }
    }

    fn warnings(&self) -> &[scarletbook::Warning] {
        match &self.source {
            Source::Range(c) => c.warnings(),
            Source::List(c, _) => c.warnings(),
        }
    }

    fn progress(&mut self) -> Progress {
        match &mut self.source {
            Source::Range(c) => c.progress(),
            Source::List(c, _) => c.progress(),
        }
    }
}

/// The cancellation request of a disc's reads (`sb_disc_cancel_token`): one per image,
/// shared by the disc, its clones and their track and raw frames handles. Unlike those
/// handles it may be used by several threads at once.
#[derive(Debug)]
pub struct SbCancel {
    token: CancelToken,
}

/// How far a track audio or raw frames handle has read (`sb_track_audio_progress`,
/// `sb_raw_frames_progress`). Unlike those handles it may be used by several threads at
/// once, while the handle reads.
#[derive(Debug)]
pub struct SbProgress {
    progress: Progress,
}

/// A seek index of one area (`sb_seek_index_new`, `sb_seek_index_load`): the area's
/// reading state every `interval` sectors, so that a range opened with it
/// (`sb_range_audio_open_indexed`) starts near its first frame instead of at the area's
/// first sector. It belongs to no disc handle: a disc and its clones (`sb_disc_clone`),
/// which read the same image, can all use it.
///
/// Keep one index per image. Its key (the area and the image's sector count) cannot tell
/// apart two images of the same size and layout, such as two rips of one disc, and an
/// index fed from both would mix their state. Within a process an index is therefore of
/// the image of the first handle it takes checkpoints from (`sb_track_audio_absorb`,
/// `sb_raw_frames_absorb`, `sb_seek_index_extend`), and refuses handles and discs of any
/// other image with `SB_STATUS_INVALID_ARGUMENT`. That identity is not saved: a loaded
/// index (`sb_seek_index_load`) takes the image of its first absorb or extend, and is
/// trusted to be of it (loading is safe, not exact).
#[derive(Debug)]
pub struct SbSeekIndex {
    inner: RefCell<AreaIndex>,
    /// Verdicts taken from the index and not yet returned by `sb_seek_index_event_next`.
    events: RefCell<std::collections::VecDeque<scarletbook::SeekEvent>>,
}

// Handles may be moved between threads (the conventions allow it); this must keep
// compiling.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<SbDisc>();
    assert_send::<SbTrackAudio>();
    assert_send::<SbRawFrames>();
    assert_send::<SbSeekIndex>();
    const fn assert_sync<T: Sync>() {}
    assert_send::<SbPool>();
    assert_sync::<SbPool>();
    // Used from any thread, by several at once.
    assert_send::<SbCancel>();
    assert_sync::<SbCancel>();
    assert_send::<SbProgress>();
    assert_sync::<SbProgress>();
};

trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

/// What a disc reads through, as far as a clone (`sb_disc_clone`) can share it.
#[derive(Clone)]
enum DiscSource {
    /// A file read at offsets: the disc and its clones each read with a position of their own.
    #[cfg(any(unix, windows))]
    File(Arc<File>),
    /// An image copied into memory by `sb_disc_open_memory`.
    Memory(Arc<Vec<u8>>),
    /// A positional read callback (`sb_disc_open_callbacks_at`).
    At(Arc<AtCallbacks>),
    /// A reader with a single position (read and seek callbacks): not shared.
    Single,
}

impl DiscSource {
    /// A new reader of the source, for a clone; `None` for a single-position reader.
    fn reader(&self) -> Option<Reader> {
        match self {
            #[cfg(any(unix, windows))]
            DiscSource::File(file) => Some(Box::new(ReadAtReader::new(Arc::clone(file)))),
            DiscSource::Memory(bytes) => Some(Box::new(ReadAtReader::new(Arc::clone(bytes)))),
            DiscSource::At(callbacks) => Some(Box::new(ReadAtReader::new(Arc::clone(callbacks)))),
            DiscSource::Single => None,
        }
    }
}

static NEXT_DISC_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::default());
}

/// A failure: a status and a message.
struct Failure(SbStatus, String);

impl Failure {
    fn arg(msg: &str) -> Self {
        Failure(SbStatus::InvalidArgument, msg.to_owned())
    }
}

impl From<Error> for Failure {
    fn from(e: Error) -> Self {
        let status = match &e {
            Error::NotScarletbook => SbStatus::NotScarletbook,
            Error::MasterTocTruncated => SbStatus::MasterTocTruncated,
            Error::MasterTocVersionUnsupported { .. } => SbStatus::MasterTocVersionUnsupported,
            Error::NoAudioAreas => SbStatus::NoAudioAreas,
            Error::ChannelCountUnsupported { .. } => SbStatus::ChannelCountUnsupported,
            Error::TrackEmpty => SbStatus::TrackEmpty,
            Error::TrackExceedsCapacity { .. } => SbStatus::TrackExceedsCapacity,
            Error::NoSuchArea(_) => SbStatus::NoSuchArea,
            Error::NoSuchTrack { .. } => SbStatus::NoSuchTrack,
            Error::FrameRangeInvalid { .. } => SbStatus::FrameRangeInvalid,
            Error::InvalidArgument(_) => SbStatus::InvalidArgument,
            Error::Io(_) => SbStatus::Io,
            Error::Cancelled => SbStatus::Cancelled,
            _ => SbStatus::Panic,
        };
        Failure(status, e.to_string())
    }
}

fn set_error(msg: &str) {
    let c = CString::new(msg.replace('\0', " ")).unwrap_or_default();
    // Once the thread's message is destroyed (a call from a thread-local destructor while
    // the thread exits), the message is dropped: `with` would panic outside `guard`.
    let _ = LAST_ERROR.try_with(|e| *e.borrow_mut() = c);
}

/// Empties the thread's message. Allocates nothing when it is already empty, as it is after
/// every successful call: this runs on every call (#132).
fn clear_error() {
    let _ = LAST_ERROR.try_with(|e| {
        if let Ok(mut e) = e.try_borrow_mut() {
            if !e.is_empty() {
                *e = CString::default();
            }
        }
    });
}

/// Runs `f`, catching panics and recording the error message.
fn guard(f: impl FnOnce() -> Result<SbStatus, Failure>) -> SbStatus {
    clear_error();
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(status)) => status,
        Ok(Err(Failure(status, msg))) => {
            set_error(&msg);
            status
        }
        Err(_) => {
            set_error("internal error: panic in scarletbook (this is a bug)");
            SbStatus::Panic
        }
    }
}

const REENTRANT: &str = "the handle is in use by the call that invoked this callback";

/// Writes `value` through an output pointer.
///
/// # Safety
///
/// `out` must be NULL or valid for a write of `T`.
unsafe fn write_out<T>(out: *mut T, value: T) -> Result<(), Failure> {
    if out.is_null() {
        return Err(Failure::arg("NULL output pointer"));
    }
    // SAFETY: non-NULL, and valid for a write of T by this function's contract.
    unsafe { out.write(value) };
    Ok(())
}

/// Sets a handle output pointer to NULL, so that it is defined if the call fails.
///
/// # Safety
///
/// `out` must be NULL or valid for a write.
unsafe fn clear_out<T>(out: *mut *mut T) {
    if !out.is_null() {
        // SAFETY: non-NULL and valid for a write by this function's contract.
        unsafe { out.write(std::ptr::null_mut()) };
    }
}

/// Borrows a seek index handle.
///
/// # Safety
///
/// `index` must be NULL or a live handle from `sb_seek_index_new` or `sb_seek_index_load`.
unsafe fn index_handle<'a>(index: *const SbSeekIndex) -> Result<&'a SbSeekIndex, Failure> {
    // SAFETY: NULL or a live handle by this function's contract; only shared references
    // to it are ever made.
    unsafe { index.as_ref() }.ok_or_else(|| Failure::arg("NULL seek index handle"))
}

/// Borrows a disc handle.
///
/// # Safety
///
/// `disc` must be NULL or a live handle from `sb_disc_open_*`.
unsafe fn disc_handle<'a>(disc: *const SbDisc) -> Result<&'a SbDisc, Failure> {
    // SAFETY: NULL or a live handle by this function's contract; only shared references
    // to it are ever made, so this one cannot alias a mutable one.
    unsafe { disc.as_ref() }.ok_or_else(|| Failure::arg("NULL disc handle"))
}

/// Borrows a track audio handle.
///
/// # Safety
///
/// `audio` must be NULL or a live handle from `sb_track_audio_open`.
unsafe fn audio_handle<'a>(audio: *const SbTrackAudio) -> Result<&'a SbTrackAudio, Failure> {
    // SAFETY: NULL or a live handle by this function's contract; only shared references
    // to it are ever made.
    unsafe { audio.as_ref() }.ok_or_else(|| Failure::arg("NULL track audio handle"))
}

/// Borrows a raw frames handle.
///
/// # Safety
///
/// `raw` must be NULL or a live handle from `sb_raw_frames_open`.
unsafe fn raw_handle<'a>(raw: *const SbRawFrames) -> Result<&'a SbRawFrames, Failure> {
    // SAFETY: NULL or a live handle by this function's contract; only shared references
    // to it are ever made.
    unsafe { raw.as_ref() }.ok_or_else(|| Failure::arg("NULL raw frames handle"))
}

impl SbDisc {
    fn get(&self) -> Result<Ref<'_, Disc<Reader>>, Failure> {
        self.inner.try_borrow().map_err(|_| Failure::arg(REENTRANT))
    }

    fn get_mut(&self) -> Result<RefMut<'_, Disc<Reader>>, Failure> {
        self.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))
    }
}

/// The disc's `SbDiscInfo` (`sb_disc_info`, `sb_disc_toc`).
fn disc_info(d: &Disc<Reader>) -> SbDiscInfo {
    let toc = d.toc();
    let m = &toc.master;
    SbDiscInfo {
        sectors: toc.sectors,
        sector_size: toc.sector_format.stored_size() as u32,
        version_major: m.version.major,
        version_minor: m.version.minor,
        album_set_size: m.album_set_size,
        album_sequence_number: m.album_sequence_number,
        hybrid: u8::from(m.hybrid()),
        manufacturer_sector: u8::from(m.manufacturer.is_some()),
        year: m.date.year,
        month: m.date.month,
        day: m.date.day,
        area_count: toc.areas.len() as u32,
        warning_count: u32::try_from(d.warnings().len()).unwrap_or(u32::MAX),
    }
}

/// Disc string `field` of `sb_disc_string` (`None`: absent).
fn disc_string(m: &MasterToc, field: u32) -> Result<Option<&str>, Failure> {
    Ok(match field {
        16 => Some(m.album_catalog_number.as_str()),
        17 => Some(m.disc_catalog_number.as_str()),
        f => {
            let item =
                DiscTextItem::ALL.get(f as usize).ok_or_else(|| Failure::arg("unknown disc string"))?;
            m.text.get(*item)
        }
    })
}

/// An area's `SbAreaInfo` (`sb_disc_area`, `sb_disc_toc`).
fn area_info(a: &Area) -> SbAreaInfo {
    SbAreaInfo {
        slot: slot_index(a.slot),
        kind_channels: match a.kind {
            scarletbook::AreaKind::TwoChannel => 2,
            scarletbook::AreaKind::Multichannel => 6,
        },
        toc_lsn: a.toc_lsn,
        toc_sectors: u32::from(a.toc_sectors),
        channel_count: u32::from(a.channel_count),
        area_config: u32::from(a.area_config),
        sample_frequency_code: u32::from(a.sample_frequency_code),
        frame_format: u32::from(a.frame_format),
        max_byte_rate: a.max_byte_rate,
        playtime_frames: a.playtime_frames(),
        audio_start_lsn: a.audio_start_lsn,
        audio_end_lsn: a.audio_end_lsn,
        track_count: a.tracks.len() as u32,
    }
}

/// An area's `SbAreaDetails` (`sb_area_details`, `sb_disc_toc`).
fn area_details(a: &Area) -> SbAreaDetails {
    SbAreaDetails {
        version_major: a.version.major,
        version_minor: a.version.minor,
        max_available_channels: a.max_available_channels,
        mute_flags: a.area_mute_flags,
        track_attribute: a.track_attribute,
        track_offset: a.track_offset,
        text_channel_count: a.text_channel_count,
        text_charset: a.text_charset,
    }
}

/// An area's `SB_LAYOUT_*` value (`sb_area_loudspeaker_layout`, `sb_disc_toc`).
fn layout_value(a: &Area) -> u32 {
    match a.loudspeaker_layout() {
        LoudspeakerLayout::Stereo => SB_LAYOUT_STEREO,
        LoudspeakerLayout::ThreeTwo => SB_LAYOUT_THREE_TWO,
        LoudspeakerLayout::FivePointOne => SB_LAYOUT_FIVE_POINT_ONE,
        LoudspeakerLayout::Unspecified { .. } => SB_LAYOUT_UNSPECIFIED,
    }
}

/// The `SB_SPEAKER_*` value of channel `index` of an area (`sb_area_channel_speaker`,
/// `sb_disc_toc`).
fn speaker_value(a: &Area, index: usize) -> u32 {
    match a.channel_speakers().and_then(|s| s.get(index)) {
        None => SB_SPEAKER_UNSPECIFIED,
        Some(Speaker::FrontLeft) => SB_SPEAKER_FRONT_LEFT,
        Some(Speaker::FrontRight) => SB_SPEAKER_FRONT_RIGHT,
        Some(Speaker::Centre) => SB_SPEAKER_CENTRE,
        Some(Speaker::LowFrequencyEffects) => SB_SPEAKER_LOW_FREQUENCY_EFFECTS,
        Some(Speaker::SurroundLeft) => SB_SPEAKER_SURROUND_LEFT,
        Some(Speaker::SurroundRight) => SB_SPEAKER_SURROUND_RIGHT,
    }
}

/// A track's `SbTrackInfo` (`sb_track_info`, `sb_disc_toc`).
fn track_info(t: &Track) -> SbTrackInfo {
    SbTrackInfo {
        number: u32::from(t.number),
        start_lsn: t.start_lsn,
        length_sectors: t.length_sectors,
        start_frame: t.start_frame,
        duration_frames: t.duration_frames,
        start_flags: t.start_flags,
        duration_flags: t.duration_flags,
        has_genre: u8::from(t.genre.is_some()),
        genre_category: t.genre.map_or(0, |g| g.category),
        genre_index: t.genre.map_or(0, |g| g.index),
        reserved: [0; 3],
    }
}

/// Track string `field` of `sb_track_string` (`None`: absent).
fn track_string(t: &Track, field: u32) -> Result<Option<&str>, Failure> {
    if field == 14 {
        return Ok(t.isrc.as_deref());
    }
    let item = TrackTextItem::ALL.get(field as usize).ok_or_else(|| Failure::arg("unknown track string"))?;
    Ok(t.text.get(*item))
}

/// Copies a string out (see the conventions in the header).
///
/// # Safety
///
/// `buf` must be NULL or valid for `cap` bytes of writes; `len` NULL or valid for a write.
unsafe fn copy_string(
    s: Option<&str>,
    buf: *mut c_char,
    cap: usize,
    len: *mut usize,
) -> Result<SbStatus, Failure> {
    // SAFETY: forwarded contract.
    unsafe { copy_bytes(s.map(str::as_bytes), buf, cap, len) }
}

/// Copies bytes out as a string is (see the conventions in the header), with a terminating
/// NUL.
///
/// # Safety
///
/// As `copy_string`.
unsafe fn copy_bytes(
    bytes: Option<&[u8]>,
    buf: *mut c_char,
    cap: usize,
    len: *mut usize,
) -> Result<SbStatus, Failure> {
    // SAFETY: forwarded contract; `len` is required whatever the string.
    unsafe { write_out(len, bytes.map_or(0, <[u8]>::len)) }?;
    let Some(bytes) = bytes else { return Ok(SbStatus::Absent) };
    if buf.is_null() || cap <= bytes.len() {
        return Ok(SbStatus::BufferTooSmall);
    }
    // SAFETY: buf is valid for cap > bytes.len() bytes; the regions cannot overlap since
    // `bytes` is owned by this library.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf.cast::<u8>(), bytes.len()) };
    // SAFETY: bytes.len() < cap, so the offset stays inside the buffer.
    let end = unsafe { buf.add(bytes.len()) };
    // SAFETY: `end` points inside the buffer, which is valid for writes.
    unsafe { end.write(0) };
    Ok(SbStatus::Ok)
}

fn slot_from(slot: u32) -> Result<Slot, Failure> {
    match slot {
        0 => Ok(Slot::TwoChannel),
        1 => Ok(Slot::Multichannel),
        _ => Err(Failure::arg("slot must be 0 (2-channel) or 1 (multichannel)")),
    }
}

fn slot_index(slot: Slot) -> u32 {
    match slot {
        Slot::TwoChannel => 0,
        Slot::Multichannel => 1,
    }
}

fn find_area(disc: &Disc<Reader>, slot: u32) -> Result<&Area, Failure> {
    let slot = slot_from(slot)?;
    disc.toc().area(slot).ok_or_else(|| Error::NoSuchArea(slot).into())
}

/// Opens a disc from a reader; `*out` must already be cleared by the caller.
///
/// # Safety
///
/// `out` must be NULL or valid for a write.
unsafe fn open_reader(
    reader: Reader,
    source: DiscSource,
    log: DiscLog,
    out: *mut *mut SbDisc,
) -> Result<SbStatus, Failure> {
    if out.is_null() {
        return Err(Failure::arg("NULL output pointer"));
    }
    let DiscLog(log, target) = log;
    let disc = match Disc::open_with_log(reader, log) {
        Ok(disc) => disc,
        Err(e) => {
            if let Some(t) = target {
                t.clear();
            }
            return Err(e.into());
        }
    };
    let handle = Box::new(SbDisc {
        inner: RefCell::new(disc),
        id: NEXT_DISC_ID.fetch_add(1, Ordering::Relaxed),
        source,
        log: RefCell::new(target),
        events: RefCell::default(),
    });
    // SAFETY: `out` is non-NULL and valid for a write by this function's contract.
    unsafe { out.write(Box::into_raw(handle)) };
    Ok(SbStatus::Ok)
}

/// The version of this C ABI. It is incremented on every breaking change: a function,
/// struct layout, enum value or macro of this header removed or changed, or a documented
/// behaviour changed incompatibly. Additions (new functions, types, enum values, macros)
/// keep it. 1 is the first released ABI; 2 made statuses, sizes and flags fixed-width
/// integers and the structs free of padding, and bounds `sb_dst_decode`'s input. The
/// compatibility policy, and what changed from 1 to 2, are in the repository's README
/// ("ABI compatibility").
pub const SB_ABI_VERSION: u32 = 2;

/// The ABI version the library was built with; compare it with `SB_ABI_VERSION` from the
/// header you compiled against.
#[unsafe(no_mangle)]
pub extern "C" fn sb_abi_version() -> u32 {
    SB_ABI_VERSION
}

/// The library version, a static NUL-terminated string.
#[unsafe(no_mangle)]
pub extern "C" fn sb_version() -> *const c_char {
    // The crate version, so that it cannot drift from the release.
    const VERSION: &CStr =
        match CStr::from_bytes_with_nul(concat!(env!("CARGO_PKG_VERSION"), "\0").as_bytes()) {
            Ok(v) => v,
            Err(_) => c"unknown",
        };
    VERSION.as_ptr()
}

/// How the library was built, a static NUL-terminated string of space-separated
/// `key=value` pairs, values without spaces, in this order:
///
/// - `version`: as `sb_version()`;
/// - `target`: the target triple, for example `x86_64-unknown-linux-gnu`;
/// - `profile`: the Cargo profile (`dist` for the release assets, `release`, `dev`) and
///   `opt-level` its optimisation level;
/// - `features`: the library's enabled Cargo features, comma-separated (`parallel`), or
///   `none`;
/// - `target-features`: the CPU features the code was compiled for, comma-separated, as
///   the compiler names them (a generic x86-64 build lists `fxsr,sse,sse2`), or `none`;
/// - `rustc`: the compiler version;
/// - `commit`: the commit of this repository the library was built from:
///   `SCARLETBOOK_COMMIT` at build time, else `GITHUB_SHA` in this repository's own GitHub
///   Actions runs, else git's HEAD when the source is this repository's work tree, else
///   `unknown` (a build from a vendored copy or a package, or by another repository's CI,
///   reports no commit of its own).
///
/// More keys may be added; parse by key, not by position. The call allocates nothing and
/// does no I/O.
#[unsafe(no_mangle)]
pub extern "C" fn sb_build_info() -> *const c_char {
    // Written by build.rs.
    const INFO: &CStr = match CStr::from_bytes_with_nul(concat!(env!("SB_BUILD_INFO"), "\0").as_bytes()) {
        Ok(v) => v,
        Err(_) => c"version=unknown",
    };
    INFO.as_ptr()
}

/// The message of the calling thread's last failed call (empty if the last call
/// succeeded). Valid until the thread's next call into this library.
#[unsafe(no_mangle)]
pub extern "C" fn sb_last_error_message() -> *const c_char {
    // Empty once the thread's message is destroyed, as for `set_error`.
    LAST_ERROR.try_with(|e| e.borrow().as_ptr()).unwrap_or(c"".as_ptr())
}

/// The name of a status code (a static string), or "unknown".
#[unsafe(no_mangle)]
pub extern "C" fn sb_status_name(status: i32) -> *const c_char {
    let name: &'static CStr = match status {
        0 => c"ok",
        1 => c"end",
        2 => c"absent",
        -1 => c"not-scarletbook",
        -2 => c"master-toc-truncated",
        -3 => c"master-toc-version-unsupported",
        -4 => c"no-audio-areas",
        -5 => c"channel-count-unsupported",
        -6 => c"track-empty",
        -7 => c"track-exceeds-capacity",
        -8 => c"no-such-area",
        -9 => c"no-such-track",
        -10 => c"invalid-argument",
        -11 => c"buffer-too-small",
        -12 => c"io-error",
        -13 => c"invalid-dst-frame",
        -14 => c"internal-error",
        -15 => c"frame-range-invalid",
        -16 => c"threads-unavailable",
        -17 => c"out-of-memory",
        -18 => c"sector-range-invalid",
        -19 => c"cancelled",
        _ => c"unknown",
    };
    name.as_ptr()
}

/// Opens a disc image file. `path` is a NUL-terminated UTF-8 path. On failure `*out` is
/// set to NULL.
///
/// # Safety
///
/// `path` must be NULL or a valid NUL-terminated string; `out` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_open_file(path: *const c_char, out: *mut *mut SbDisc) -> SbStatus {
    // SAFETY: forwarded contract; no log.
    unsafe { sb_disc_open_file_with_log(path, None, std::ptr::null_mut(), SB_LOG_OFF, out) }
}

/// `sb_disc_open_file`, reporting to a log callback from the format detection on (see
/// `sb_disc_set_log`).
///
/// # Safety
///
/// As `sb_disc_open_file` and `sb_disc_set_log`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_open_file_with_log(
    path: *const c_char,
    log: SbLogFn,
    log_user: *mut c_void,
    level: i32,
    out: *mut *mut SbDisc,
) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { clear_out(out) };
    guard(|| {
        // SAFETY: forwarded contract.
        let log = unsafe { DiscLog::new(log, log_user, level) }?;
        if path.is_null() {
            return Err(Failure::arg("NULL path"));
        }
        // SAFETY: non-NULL and NUL-terminated by this function's contract.
        let path = unsafe { CStr::from_ptr(path) };
        let path = path.to_str().map_err(|_| Failure::arg("path is not UTF-8"))?;
        let file = File::open(path).map_err(|e| {
            log.0.write(Level::Error, format_args!("{path}: {e}"));
            Failure(SbStatus::Io, format!("{path}: {e}"))
        })?;
        // Read at offsets where the system offers it, so that clones share the file.
        #[cfg(any(unix, windows))]
        let (reader, source): (Reader, DiscSource) = {
            let file = Arc::new(file);
            (Box::new(ReadAtReader::new(Arc::clone(&file))), DiscSource::File(file))
        };
        #[cfg(not(any(unix, windows)))]
        let (reader, source): (Reader, DiscSource) = (Box::new(BufReader::new(file)), DiscSource::Single);
        // SAFETY: forwarded contract.
        unsafe { open_reader(reader, source, log, out) }
    })
}

/// Opens a disc image held in memory. The bytes are copied; the caller may free `data`
/// when the call returns. On failure `*out` is set to NULL; `SB_STATUS_OUT_OF_MEMORY` if
/// the copy cannot be allocated.
///
/// # Safety
///
/// `data` must be NULL (only with `len` 0) or valid for `len` bytes of reads; `out` NULL
/// or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_open_memory(data: *const u8, len: usize, out: *mut *mut SbDisc) -> SbStatus {
    // SAFETY: forwarded contract; no log.
    unsafe { sb_disc_open_memory_with_log(data, len, None, std::ptr::null_mut(), SB_LOG_OFF, out) }
}

/// `sb_disc_open_memory`, reporting to a log callback from the format detection on (see
/// `sb_disc_set_log`).
///
/// # Safety
///
/// As `sb_disc_open_memory` and `sb_disc_set_log`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_open_memory_with_log(
    data: *const u8,
    len: usize,
    log: SbLogFn,
    log_user: *mut c_void,
    level: i32,
    out: *mut *mut SbDisc,
) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { clear_out(out) };
    guard(|| {
        // SAFETY: forwarded contract.
        let log = unsafe { DiscLog::new(log, log_user, level) }?;
        let bytes = if len == 0 {
            Vec::new()
        } else if data.is_null() {
            return Err(Failure::arg("NULL data with non-zero length"));
        } else {
            // SAFETY: non-NULL and valid for `len` bytes of reads by this function's contract.
            let src = unsafe { std::slice::from_raw_parts(data, len) };
            // The copy's size is the caller's: report a failed allocation instead of
            // aborting the process.
            let mut bytes = Vec::new();
            bytes.try_reserve_exact(len).map_err(|_| {
                Failure(SbStatus::OutOfMemory, format!("cannot allocate {len} bytes for the image copy"))
            })?;
            bytes.extend_from_slice(src);
            bytes
        };
        let bytes = Arc::new(bytes);
        // SAFETY: forwarded contract.
        unsafe {
            open_reader(Box::new(ReadAtReader::new(Arc::clone(&bytes))), DiscSource::Memory(bytes), log, out)
        }
    })
}

/// Reads up to `len` bytes into `buf`; returns the count (0 at the end) or -1 on error.
pub type SbReadFn = Option<unsafe extern "C" fn(user: *mut c_void, buf: *mut u8, len: usize) -> i64>;

/// Seeks to `offset` from `whence` (0: start, 1: current, 2: end); returns the new
/// position or -1 on error.
pub type SbSeekFn = Option<unsafe extern "C" fn(user: *mut c_void, offset: i64, whence: i32) -> i64>;

type ReadFn = unsafe extern "C" fn(*mut c_void, *mut u8, usize) -> i64;
type SeekFn = unsafe extern "C" fn(*mut c_void, i64, i32) -> i64;

struct Callbacks {
    read: ReadFn,
    seek: SeekFn,
    user: *mut c_void,
}

// SAFETY: the caller of sb_disc_open_callbacks guarantees that the callbacks and `user`
// may be used from whichever thread the disc handle is moved to (its # Safety section).
unsafe impl Send for Callbacks {}

impl Read for Callbacks {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // SAFETY: the caller of sb_disc_open_callbacks guaranteed that `read` may be
        // called with `user` and a buffer valid for `len` bytes of writes, which `buf` is.
        let n = unsafe { (self.read)(self.user, buf.as_mut_ptr(), buf.len()) };
        usize::try_from(n)
            .ok()
            .filter(|&n| n <= buf.len())
            .ok_or_else(|| io::Error::other("read callback failed or returned too many bytes"))
    }
}

impl Seek for Callbacks {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let (offset, whence) = match pos {
            SeekFrom::Start(n) => (i64::try_from(n).map_err(io::Error::other)?, 0),
            SeekFrom::Current(n) => (n, 1),
            SeekFrom::End(n) => (n, 2),
        };
        // SAFETY: the caller of sb_disc_open_callbacks guaranteed that `seek` may be
        // called with `user`.
        let r = unsafe { (self.seek)(self.user, offset, whence) };
        u64::try_from(r).map_err(|_| io::Error::other("seek callback failed"))
    }
}

/// Reads up to `len` bytes at byte `offset` of the image into `buf`; returns the count (0
/// at or past the end) or -1 on error. May be called from several threads at once (see
/// `sb_disc_open_callbacks_at`).
pub type SbReadAtFn =
    Option<unsafe extern "C" fn(user: *mut c_void, offset: u64, buf: *mut u8, len: usize) -> i64>;

type ReadAtFn = unsafe extern "C" fn(*mut c_void, u64, *mut u8, usize) -> i64;

/// A positional read callback, shared by a disc and its clones.
struct AtCallbacks {
    read_at: ReadAtFn,
    size: u64,
    user: *mut c_void,
}

// SAFETY: the caller of sb_disc_open_callbacks_at guarantees that `read_at` may be called
// with `user` from any thread, concurrently, until the disc and its clones are freed (its
// # Safety section); `size` is plain data.
unsafe impl Send for AtCallbacks {}
// SAFETY: as above: calls through shared references, concurrent ones included, are allowed.
unsafe impl Sync for AtCallbacks {}

impl ReadAt for AtCallbacks {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        // SAFETY: the caller of sb_disc_open_callbacks_at guaranteed that `read_at` may be
        // called with `user`, from any thread, concurrently, and a buffer valid for `len`
        // bytes of writes, which `buf` is.
        let n = unsafe { (self.read_at)(self.user, offset, buf.as_mut_ptr(), buf.len()) };
        usize::try_from(n)
            .ok()
            .filter(|&n| n <= buf.len())
            .ok_or_else(|| io::Error::other("read callback failed or returned too many bytes"))
    }

    fn size(&self) -> io::Result<u64> {
        Ok(self.size)
    }
}

/// Opens a disc image read through callbacks. On failure `*out` is set to NULL.
///
/// The callbacks run during this call and during `sb_track_audio_read`,
/// `sb_raw_frames_read` and `sb_disc_read_sectors`, on the calling thread. A callback that calls back into this
/// library with this disc or one of its track audio or raw frames handles gets
/// `SB_STATUS_INVALID_ARGUMENT` (and their _free functions do nothing); other handles may
/// be used.
///
/// # Safety
///
/// `read` and `seek` must be safe to call with `user`, from any thread the disc is moved
/// to, until the disc is freed; they must not unwind; `read` must write at most `len`
/// bytes to `buf`. `out` must be NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_open_callbacks(
    read: SbReadFn,
    seek: SbSeekFn,
    user: *mut c_void,
    out: *mut *mut SbDisc,
) -> SbStatus {
    // SAFETY: forwarded contract; no log.
    unsafe { sb_disc_open_callbacks_with_log(read, seek, user, None, std::ptr::null_mut(), SB_LOG_OFF, out) }
}

/// `sb_disc_open_callbacks`, reporting to a log callback from the format detection on
/// (see `sb_disc_set_log`). `user` goes to the read and seek callbacks, `log_user` to the
/// log callback.
///
/// # Safety
///
/// As `sb_disc_open_callbacks` and `sb_disc_set_log`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_open_callbacks_with_log(
    read: SbReadFn,
    seek: SbSeekFn,
    user: *mut c_void,
    log: SbLogFn,
    log_user: *mut c_void,
    level: i32,
    out: *mut *mut SbDisc,
) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { clear_out(out) };
    guard(|| {
        // SAFETY: forwarded contract.
        let log = unsafe { DiscLog::new(log, log_user, level) }?;
        let (Some(read), Some(seek)) = (read, seek) else {
            return Err(Failure::arg("NULL callback"));
        };
        // SAFETY: forwarded contract.
        unsafe { open_reader(Box::new(Callbacks { read, seek, user }), DiscSource::Single, log, out) }
    })
}

/// Opens a disc image of `size` bytes read through a positional read callback, which is
/// given the offset of each read and so can serve the disc and its clones (`sb_disc_clone`)
/// on several threads at once. On failure `*out` is set to NULL.
///
/// The callback runs during this call and during `sb_track_audio_read` and
/// `sb_raw_frames_read`, on the calling thread, with the rules of `sb_disc_open_callbacks`
/// for calling back into this library; clones of the disc used on several threads call it
/// on those threads at the same time. `size` is the image's length (what `seek` to the end
/// gives for `sb_disc_open_callbacks`): a short count or -1 fails the read that needed it
/// (`SB_STATUS_IO`).
///
/// # Safety
///
/// `read_at` must be safe to call with `user` from any thread, concurrently, until the
/// disc and all its clones are freed; it must not unwind, and must write at most `len`
/// bytes to `buf`. `out` must be NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_open_callbacks_at(
    read_at: SbReadAtFn,
    size: u64,
    user: *mut c_void,
    out: *mut *mut SbDisc,
) -> SbStatus {
    // SAFETY: forwarded contract; no log.
    unsafe { sb_disc_open_callbacks_at_with_log(read_at, size, user, None, ptr::null_mut(), SB_LOG_OFF, out) }
}

/// `sb_disc_open_callbacks_at`, reporting to a log callback from the format detection on
/// (see `sb_disc_set_log`). `user` goes to the read callback, `log_user` to the log
/// callback.
///
/// # Safety
///
/// As `sb_disc_open_callbacks_at` and `sb_disc_set_log`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_open_callbacks_at_with_log(
    read_at: SbReadAtFn,
    size: u64,
    user: *mut c_void,
    log: SbLogFn,
    log_user: *mut c_void,
    level: i32,
    out: *mut *mut SbDisc,
) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { clear_out(out) };
    guard(|| {
        // SAFETY: forwarded contract.
        let log = unsafe { DiscLog::new(log, log_user, level) }?;
        let Some(read_at) = read_at else {
            if let Some(t) = &log.1 {
                t.clear();
            }
            return Err(Failure::arg("NULL callback"));
        };
        let callbacks = Arc::new(AtCallbacks { read_at, size, user });
        let reader = Box::new(ReadAtReader::new(Arc::clone(&callbacks)));
        // SAFETY: forwarded contract.
        unsafe { open_reader(reader, DiscSource::At(callbacks), log, out) }
    })
}

/// Opens a clone of a disc: the same image with a reading position of its own, sharing
/// the disc's parsed TOC, warnings and sector format, without the log callback (see
/// `sb_disc_clone_with_log`). For reads of one image on several threads at once, give each
/// thread a clone: each is a handle of its own, used by one thread at a time as any handle.
/// On failure `*out` is set to NULL.
///
/// A disc and its clones read the same image: a track audio or raw frames handle opened
/// from any of them may be read through any other (`sb_track_audio_read`,
/// `sb_raw_frames_read`), and continued through any other (`_open_next`). Each may be freed
/// before or after the others.
///
/// Discs opened from a file or from memory can be cloned; reads through a file use the
/// same file handle, at their own offsets (on Windows, the system may serialise reads
/// through one handle). A disc opened with read and seek callbacks has a single position:
/// cloning it fails with `SB_STATUS_INVALID_ARGUMENT`. The clone checks the image again
/// (its length and TOC sectors): `SB_STATUS_IO` if that read fails.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `out` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_clone(disc: *const SbDisc, out: *mut *mut SbDisc) -> SbStatus {
    // SAFETY: forwarded contract; no log.
    unsafe { sb_disc_clone_with_log(disc, None, std::ptr::null_mut(), SB_LOG_OFF, out) }
}

/// `sb_disc_clone`, with the clone reporting to a log callback of its own from the check
/// of the image on (see `sb_disc_set_log`). The callback is the clone's: replacing or
/// freeing the original's does not affect it.
///
/// # Safety
///
/// As `sb_disc_clone` and `sb_disc_set_log`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_clone_with_log(
    disc: *const SbDisc,
    log: SbLogFn,
    log_user: *mut c_void,
    level: i32,
    out: *mut *mut SbDisc,
) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { clear_out(out) };
    guard(|| {
        // SAFETY: forwarded contract.
        let log = unsafe { DiscLog::new(log, log_user, level) }?;
        // SAFETY: forwarded contract.
        unsafe { clone_into(disc, None, log, out) }
    })
}

/// The log user data of a function without a log callback.
const NO_USER: *mut c_void = ptr::null_mut();

/// The shared body of the `sb_disc_clone` functions: a clone of `disc` reading through
/// `reader` (with what a clone of it would read through), or by default through a new
/// reader of the disc's source.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `out` NULL or valid for a write and already
/// cleared by the caller.
unsafe fn clone_into(
    disc: *const SbDisc,
    reader: Option<(Reader, DiscSource)>,
    log: DiscLog,
    out: *mut *mut SbDisc,
) -> Result<SbStatus, Failure> {
    let DiscLog(log, target) = log;
    // The new callback is called only by the clone: stop it on every failure.
    let clear = |e: Failure| {
        if let Some(t) = &target {
            t.clear();
        }
        e
    };
    // SAFETY: forwarded contract.
    let handle = unsafe { disc_handle(disc) }.map_err(clear)?;
    if out.is_null() {
        return Err(clear(Failure::arg("NULL output pointer")));
    }
    let (reader, source) = match reader {
        Some(given) => given,
        None => match handle.source.reader() {
            Some(reader) => (reader, handle.source.clone()),
            None => {
                return Err(clear(Failure::arg(
                    "a disc opened with read and seek callbacks cannot be cloned (see sb_disc_clone_callbacks)",
                )));
            }
        },
    };
    let d = handle.get().map_err(clear)?;
    let clone = d.share_with_log(reader, log).map_err(|e| clear(e.into()))?;
    let clone = Box::new(SbDisc {
        inner: RefCell::new(clone),
        id: handle.id,
        source,
        log: RefCell::new(target),
        events: RefCell::default(),
    });
    // SAFETY: non-NULL and valid for a write by this function's contract.
    unsafe { out.write(Box::into_raw(clone)) };
    Ok(SbStatus::Ok)
}

/// Opens a clone of any disc read through new read and seek callbacks (see
/// `sb_disc_open_callbacks`), for example over a second handle of the image's file: as
/// `sb_disc_clone`, with the clone reading through `read` and `seek`. The reader is
/// checked: it must have the image's length and the same TOC sectors, else
/// `SB_STATUS_INVALID_ARGUMENT`; that cannot prove every sector the same, which is the
/// caller's responsibility. The clone has a single position: `sb_disc_clone` refuses it,
/// while `sb_disc_clone_callbacks` takes it. On failure `*out` is set to NULL.
///
/// # Safety
///
/// As `sb_disc_clone` for `disc` and `out`, and as `sb_disc_open_callbacks` for `read`,
/// `seek` and `user`, until the clone is freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_clone_callbacks(
    disc: *const SbDisc,
    read: SbReadFn,
    seek: SbSeekFn,
    user: *mut c_void,
    out: *mut *mut SbDisc,
) -> SbStatus {
    // SAFETY: forwarded contract; no log.
    unsafe { sb_disc_clone_callbacks_with_log(disc, read, seek, user, None, NO_USER, SB_LOG_OFF, out) }
}

/// `sb_disc_clone_callbacks`, with the clone reporting to a log callback of its own (see
/// `sb_disc_clone_with_log`). `user` goes to the read and seek callbacks, `log_user` to the
/// log callback.
///
/// # Safety
///
/// As `sb_disc_clone_callbacks` and `sb_disc_set_log`.
#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_clone_callbacks_with_log(
    disc: *const SbDisc,
    read: SbReadFn,
    seek: SbSeekFn,
    user: *mut c_void,
    log: SbLogFn,
    log_user: *mut c_void,
    level: i32,
    out: *mut *mut SbDisc,
) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { clear_out(out) };
    guard(|| {
        // SAFETY: forwarded contract.
        let log = unsafe { DiscLog::new(log, log_user, level) }?;
        let (Some(read), Some(seek)) = (read, seek) else {
            if let Some(t) = &log.1 {
                t.clear();
            }
            return Err(Failure::arg("NULL callback"));
        };
        let reader: Reader = Box::new(Callbacks { read, seek, user });
        // SAFETY: forwarded contract.
        unsafe { clone_into(disc, Some((reader, DiscSource::Single)), log, out) }
    })
}

/// Log levels, most severe first: the `level` of a message and the most verbose level a
/// log callback receives. `SB_LOG_OFF` as a `level` argument turns the log off.
pub const SB_LOG_OFF: i32 = 0;
/// The reader failed, or decoding stopped on an internal error.
pub const SB_LOG_ERROR: i32 = 1;
/// A problem in the image: a warning (also reported by `sb_disc_warning` and
/// `sb_track_audio_warning`), a malformed sector or a missing, invalid or discarded frame,
/// with its LSN or frame number.
pub const SB_LOG_WARN: i32 = 2;
/// What was found: the sector format, the TOC copies used, the areas, the tracks opened.
pub const SB_LOG_INFO: i32 = 3;
/// What was done: every read (LSN, sector count, byte range), the format probes, the
/// master TOC copy comparisons, the area scans.
pub const SB_LOG_DEBUG: i32 = 4;
/// Every frame returned, every frame the reassembler accepts, and each group of DST
/// frames decoded.
pub const SB_LOG_TRACE: i32 = 5;

/// Receives one log message: `level` is one of `SB_LOG_ERROR` to `SB_LOG_TRACE`, `message`
/// a NUL-terminated UTF-8 line without a trailing newline, valid only during the call.
pub type SbLogFn = Option<unsafe extern "C" fn(user: *mut c_void, level: i32, message: *const c_char)>;

type LogFn = unsafe extern "C" fn(*mut c_void, i32, *const c_char);

/// A disc's C log callback, shared with the decoding jobs that report to it, until it is
/// cleared.
struct LogTarget {
    state: Mutex<LogState>,
    idle: Condvar,
}

struct LogState {
    /// The callback and its user data; `None` once cleared.
    callback: Option<(LogFn, *mut c_void)>,
    /// Callbacks running now.
    running: usize,
}

// SAFETY: the caller of sb_disc_set_log and the _with_log openers guarantees that the
// callback may be called with its user data from any thread, concurrently (their # Safety
// sections); the rest is plain data under the mutex.
unsafe impl Send for LogState {}

impl LogTarget {
    fn lock(&self) -> MutexGuard<'_, LogState> {
        // No panic happens while the lock is held: the state stays consistent.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Calls the callback with a message, unless it has been cleared. The lock is not held
    /// during the call, so the callback may run on several threads at once.
    fn emit(&self, level: Level, message: std::fmt::Arguments<'_>) {
        let Some((callback, user)) = ({
            let mut state = self.lock();
            let callback = state.callback;
            if callback.is_some() {
                state.running += 1;
            }
            callback
        }) else {
            return;
        };
        /// Counts the call as finished however it ends.
        struct Running<'a>(&'a LogTarget);
        impl Drop for Running<'_> {
            fn drop(&mut self) {
                let mut state = self.0.lock();
                state.running -= 1;
                if state.running == 0 {
                    self.0.idle.notify_all();
                }
            }
        }
        let _running = Running(self);
        // Formatted only once the callback is known to be set.
        let text = CString::new(message.to_string().replace('\0', " ")).unwrap_or_default();
        // SAFETY: the callback may be called with its user data from any thread until it
        // is cleared (the setter's contract); clear() waits for this call to return.
        // `text` is a valid NUL-terminated string that outlives the call.
        unsafe { callback(user, level as i32, text.as_ptr()) };
    }

    /// Stops the callback: returns once no call is running and none will start.
    fn clear(&self) {
        let mut state = self.lock();
        state.callback = None;
        while state.running > 0 {
            state = self.idle.wait(state).unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// A disc's core log and the C callback behind it, from the arguments of a setter.
struct DiscLog(Log, Option<Arc<LogTarget>>);

impl DiscLog {
    /// Checks the level and builds the log; no log for a NULL callback or `SB_LOG_OFF`.
    ///
    /// # Safety
    ///
    /// `callback` must be safe to call with `user` from any thread, concurrently, until
    /// the log is replaced or the disc freed.
    unsafe fn new(callback: SbLogFn, user: *mut c_void, level: i32) -> Result<Self, Failure> {
        let level = match level {
            SB_LOG_OFF => None,
            SB_LOG_ERROR => Some(Level::Error),
            SB_LOG_WARN => Some(Level::Warn),
            SB_LOG_INFO => Some(Level::Info),
            SB_LOG_DEBUG => Some(Level::Debug),
            SB_LOG_TRACE => Some(Level::Trace),
            _ => return Err(Failure::arg("log level must be SB_LOG_OFF (0) to SB_LOG_TRACE (5)")),
        };
        let (Some(callback), Some(level)) = (callback, level) else {
            return Ok(DiscLog(Log::none(), None));
        };
        let target = Arc::new(LogTarget {
            state: Mutex::new(LogState { callback: Some((callback, user)), running: 0 }),
            idle: Condvar::new(),
        });
        let sink = Arc::clone(&target);
        Ok(DiscLog(Log::new(level, move |level, message| sink.emit(level, message)), Some(target)))
    }
}

/// Sets, replaces or removes (`log` NULL or `level` `SB_LOG_OFF`) a disc's log callback.
/// It receives the messages at `level` and more severe ones (`SB_LOG_ERROR` <
/// `SB_LOG_WARN` < ... < `SB_LOG_TRACE`), from the next call on. To also get the format
/// detection and TOC messages, open the disc with a `_with_log` function. Without a
/// callback, or above its level, no message is formatted.
///
/// Calls with this disc and its track handles invoke the callback on the calling thread
/// while they run, in order. A track handle decoding on a pool (`sb_track_audio_set_pool`;
/// in a library built with the `parallel` feature, every track handle) also invokes it
/// from the pool's threads, which decode ahead of the caller: at `SB_LOG_TRACE` (groups
/// of frames decoded) and `SB_LOG_ERROR` (an internal error), possibly concurrently with
/// other calls of it and after the call that started the work has returned; such lines
/// can interleave with the calling thread's. Frame problems are reported on the calling
/// thread, in frame order. Once sb_disc_set_log or sb_disc_free returns, the previous
/// callback is not called again (they wait for calls on other threads to return).
///
/// A pool thread's message can also be delivered on a thread that waits for a frame inside
/// `sb_track_audio_read` when that thread is one of a pool's (the pool runs pending work
/// there; for example a callback on a pool thread that reads another handle), and the callback
/// can be entered again on a thread that is already inside it (when the callback itself
/// calls this library, with any handle, and that call waits for decoding). A callback
/// that takes a non-recursive lock which it, or the caller around its calls to this
/// library, already holds can therefore deadlock.
///
/// Lines about problems are capped: at most 4096 per kind (malformed sectors and
/// discarded frames per area scan, missing and invalid frames per track), then one line
/// saying the rest are suppressed. The counts (`sb_track_audio_counts`,
/// `sb_track_audio_malformed_sectors`) are kept in full.
///
/// The message is valid only during the call. The callback must not unwind. It may call
/// this library with other handles. With this disc or its track handles, it may do so only
/// when it runs on the thread of the call that invoked it, where a call that conflicts
/// with the running one fails with `SB_STATUS_INVALID_ARGUMENT`; from a pool thread that
/// would use the handle from two threads at once (`sb_track_audio_set_pool` and the
/// `_free` functions included).
///
/// Returns `SB_STATUS_INVALID_ARGUMENT` for a level outside `SB_LOG_OFF` to
/// `SB_LOG_TRACE`, or when called from one of this disc's callbacks; the log is then
/// unchanged.
///
/// # Safety
///
/// `disc` must be NULL or a live handle. `log` must be safe to call with `user` from any
/// thread, concurrently, until it is replaced or the disc is freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_set_log(
    disc: *const SbDisc,
    log: SbLogFn,
    user: *mut c_void,
    level: i32,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let handle = unsafe { disc_handle(disc) }?;
        let mut d = handle.get_mut()?;
        // SAFETY: forwarded contract.
        let DiscLog(log, target) = unsafe { DiscLog::new(log, user, level) }?;
        d.set_log(log);
        let old = handle.log.replace(target);
        if let Some(old) = old {
            old.clear();
        }
        Ok(SbStatus::Ok)
    })
}

/// Frees a disc. NULL is ignored, and so is a disc whose callback is running (the call is
/// then a no-op). Track audio handles opened from it can afterwards only be freed, or
/// read through a clone of it (`sb_disc_clone`). Its clones stay usable.
///
/// # Safety
///
/// `disc` must be NULL or a live handle, freed only once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_free(disc: *mut SbDisc) {
    let _ = guard(|| {
        // SAFETY: forwarded contract.
        let handle = unsafe { disc_handle(disc) }?;
        if handle.inner.try_borrow_mut().is_err() {
            return Err(Failure::arg(REENTRANT));
        }
        // SAFETY: a live handle created by Box::into_raw and not in use (no borrow is
        // active, so no call on any frame holds a reference into it), freed only once.
        drop(unsafe { Box::from_raw(disc) });
        Ok(SbStatus::Ok)
    });
}

/// Disc-level information.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `out` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_info(disc: *const SbDisc, out: *mut SbDiscInfo) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?.get()?;
        // SAFETY: forwarded contract.
        unsafe { write_out(out, disc_info(&d)) }?;
        Ok(SbStatus::Ok)
    })
}

/// Copies a disc-level string (`SbDiscString`).
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `buf` NULL or valid for `cap` bytes; `len` NULL
/// or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_string(
    disc: *const SbDisc,
    field: u32,
    buf: *mut c_char,
    cap: usize,
    len: *mut usize,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?.get()?;
        let s = disc_string(&d.toc().master, field)?;
        // SAFETY: forwarded contract.
        unsafe { copy_string(s, buf, cap, len) }
    })
}

/// Information about area `index` (0 or 1, in slot order).
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `out` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_area(disc: *const SbDisc, index: u32, out: *mut SbAreaInfo) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?.get()?;
        let a = d.toc().areas.get(index as usize).ok_or_else(|| Failure::arg("area index out of range"))?;
        // SAFETY: forwarded contract.
        unsafe { write_out(out, area_info(a)) }?;
        Ok(SbStatus::Ok)
    })
}

/// Loudspeaker layouts (`sb_area_loudspeaker_layout`), from the channel count and the low 3
/// bits of the area configuration byte (specification 3, 3.4.1). Any other combination has
/// no defined layout: `SB_LAYOUT_UNSPECIFIED`, with both fields in `SbAreaInfo` as stored.
pub const SB_LAYOUT_UNSPECIFIED: u32 = 0;
/// Stereo: 2 channels, configuration 0.
pub const SB_LAYOUT_STEREO: u32 = 1;
/// 5-channel (3/2): 5 channels, configuration 3.
pub const SB_LAYOUT_THREE_TWO: u32 = 2;
/// 5.1-channel: 6 channels, configuration 4.
pub const SB_LAYOUT_FIVE_POINT_ONE: u32 = 3;

/// Loudspeakers (`sb_area_channel_speaker`). `SB_SPEAKER_UNSPECIFIED` is every channel of
/// an area without a defined layout.
pub const SB_SPEAKER_UNSPECIFIED: u32 = 0;
/// Front left; "left" in stereo.
pub const SB_SPEAKER_FRONT_LEFT: u32 = 1;
/// Front right; "right" in stereo.
pub const SB_SPEAKER_FRONT_RIGHT: u32 = 2;
/// Centre.
pub const SB_SPEAKER_CENTRE: u32 = 3;
/// Low-frequency effects.
pub const SB_SPEAKER_LOW_FREQUENCY_EFFECTS: u32 = 4;
/// Surround left.
pub const SB_SPEAKER_SURROUND_LEFT: u32 = 5;
/// Surround right.
pub const SB_SPEAKER_SURROUND_RIGHT: u32 = 6;

/// Writes the loudspeaker layout (`SB_LAYOUT_*`) of the area from `slot` to `*layout`.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `layout` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_area_loudspeaker_layout(
    disc: *const SbDisc,
    slot: u32,
    layout: *mut u32,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?.get()?;
        let value = layout_value(find_area(&d, slot)?);
        // SAFETY: forwarded contract.
        unsafe { write_out(layout, value) }?;
        Ok(SbStatus::Ok)
    })
}

/// Writes the loudspeaker (`SB_SPEAKER_*`) of channel `index` of the area from `slot` to
/// `*speaker`. Channels are numbered from 0 in the order they appear in a decoded frame,
/// up to the channel count less 1; a larger index is an invalid argument. The order is
/// informative (specification 3, 3.4.1). An area without a defined layout gives
/// `SB_SPEAKER_UNSPECIFIED` for every channel.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `speaker` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_area_channel_speaker(
    disc: *const SbDisc,
    slot: u32,
    index: u32,
    speaker: *mut u32,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?.get()?;
        let area = find_area(&d, slot)?;
        if index >= u32::from(area.channel_count) {
            return Err(Failure::arg("channel index out of range"));
        }
        let value = speaker_value(area, index as usize);
        // SAFETY: forwarded contract.
        unsafe { write_out(speaker, value) }?;
        Ok(SbStatus::Ok)
    })
}

/// Copies an area string (`SbAreaString`) of the area from `slot`.
///
/// # Safety
///
/// As `sb_disc_string`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_area_string(
    disc: *const SbDisc,
    slot: u32,
    field: u32,
    buf: *mut c_char,
    cap: usize,
    len: *mut usize,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?.get()?;
        let area = find_area(&d, slot)?;
        let item =
            AreaTextItem::ALL.get(field as usize).ok_or_else(|| Failure::arg("unknown area string"))?;
        // SAFETY: forwarded contract.
        unsafe { copy_string(area.text.get(*item), buf, cap, len) }
    })
}

/// Genre lists of `sb_disc_genre`: the master TOC's album genres (offset 40) and disc
/// genres (offset 104).
pub const SB_GENRE_LIST_ALBUM: u32 = 0;
/// See `SB_GENRE_LIST_ALBUM`.
pub const SB_GENRE_LIST_DISC: u32 = 1;

/// Genre `index` (0 to 3) of the master TOC's album (`SB_GENRE_LIST_ALBUM`) or disc
/// (`SB_GENRE_LIST_DISC`) genres, as stored.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `out` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_genre(
    disc: *const SbDisc,
    list: u32,
    index: u32,
    out: *mut SbGenre,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?.get()?;
        let m = &d.toc().master;
        let genres = match list {
            SB_GENRE_LIST_ALBUM => &m.album_genres,
            SB_GENRE_LIST_DISC => &m.disc_genres,
            _ => return Err(Failure::arg("unknown genre list")),
        };
        let g = genres.get(index as usize).ok_or_else(|| Failure::arg("genre index out of range"))?;
        // SAFETY: forwarded contract.
        unsafe { write_out(out, SbGenre { category: g.category, index: g.index }) }?;
        Ok(SbStatus::Ok)
    })
}

/// The master TOC's text channel count (offset 128), as stored (0 to 255);
/// `sb_disc_text_channel` reads the first `min(count, 8)` locale entries.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `count` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_text_channel_count(disc: *const SbDisc, count: *mut u32) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?.get()?;
        // SAFETY: forwarded contract.
        unsafe { write_out(count, u32::from(d.toc().master.text_channel_count)) }?;
        Ok(SbStatus::Ok)
    })
}

fn text_channel(locales: &[scarletbook::toc::Locale], index: u32) -> Result<SbTextChannel, Failure> {
    let l = locales.get(index as usize).ok_or_else(|| Failure::arg("text channel index out of range"))?;
    Ok(SbTextChannel { language: l.language_code, charset: l.charset, reserved: 0 })
}

/// The locale entry of master TOC text channel `index`, below `min(count, 8)` of
/// `sb_disc_text_channel_count` (the reported locale list, specification 2, 2.5.2);
/// `SB_STATUS_INVALID_ARGUMENT` for another index.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `out` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_text_channel(
    disc: *const SbDisc,
    index: u32,
    out: *mut SbTextChannel,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?.get()?;
        let channel = text_channel(&d.toc().master.locales, index)?;
        // SAFETY: forwarded contract.
        unsafe { write_out(out, channel) }?;
        Ok(SbStatus::Ok)
    })
}

/// Copies a catalogue number as stored: `field` is `SB_DISC_STRING_ALBUM_CATALOG_NUMBER`
/// or `SB_DISC_STRING_DISC_CATALOG_NUMBER`, and the bytes are the field's up to its first
/// 0x00 (all 16 if there is none), padding kept, unmapped and untrimmed (specification 2,
/// 2.6.5 step 1); `sb_disc_string` gives the trimmed string. The copy-out convention is
/// that of strings, with a terminating NUL, but the bytes are as stored: not necessarily
/// UTF-8 (they contain no 0x00). Empty when the field starts with 0x00; never absent.
///
/// # Safety
///
/// As `sb_disc_string`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_catalog_number_stored(
    disc: *const SbDisc,
    field: u32,
    buf: *mut c_char,
    cap: usize,
    len: *mut usize,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?.get()?;
        let m = &d.toc().master;
        let bytes = match field {
            f if f == SbDiscString::AlbumCatalogNumber as u32 => &m.album_catalog_number_stored,
            f if f == SbDiscString::DiscCatalogNumber as u32 => &m.disc_catalog_number_stored,
            _ => return Err(Failure::arg("field must be a catalogue number")),
        };
        // SAFETY: forwarded contract.
        unsafe { copy_bytes(Some(bytes), buf, cap, len) }
    })
}

/// Size of the manufacturer data `sb_disc_manufacturer` copies: bytes 8 to 2047 of the
/// manufacturer sector.
pub const SB_MANUFACTURER_BYTES: u32 = 2040;

/// Copies the manufacturer data: bytes 8 to 2047 of the master TOC's manufacturer sector
/// (LSN 519), opaque (specification 2, 2.7), when its `SACD_Man` signature is present
/// (`SbDiscInfo.manufacturer_sector`). `*len` (required) receives the size,
/// `SB_MANUFACTURER_BYTES`, or 0 and `SB_STATUS_ABSENT` without the sector. With `buf`
/// NULL or `cap` below the size, the call returns `SB_STATUS_BUFFER_TOO_SMALL` and
/// writes nothing else. No NUL is added.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `buf` NULL or valid for `cap` bytes of writes;
/// `len` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_manufacturer(
    disc: *const SbDisc,
    buf: *mut u8,
    cap: usize,
    len: *mut usize,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?.get()?;
        let bytes = d.toc().master.manufacturer.as_deref();
        // SAFETY: forwarded contract.
        unsafe { write_out(len, bytes.map_or(0, <[u8]>::len)) }?;
        let Some(bytes) = bytes else { return Ok(SbStatus::Absent) };
        if buf.is_null() || cap < bytes.len() {
            return Ok(SbStatus::BufferTooSmall);
        }
        // SAFETY: buf is valid for cap >= bytes.len() bytes; `bytes` is owned by the disc,
        // so the regions do not overlap.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len()) };
        Ok(SbStatus::Ok)
    })
}

/// A timecode as stored: its three bytes, minutes, seconds and frames (specification 0,
/// 0.5), not validated (a stored 255:255:255 is given as such).
///
/// Layout: 3 bytes, aligned to 1, without padding (3 fields of 1 byte).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SbTimecode {
    /// Minutes.
    pub minutes: u8,
    /// Seconds.
    pub seconds: u8,
    /// Frames.
    pub frames: u8,
}

impl From<scarletbook::toc::Timecode> for SbTimecode {
    fn from(t: scarletbook::toc::Timecode) -> Self {
        SbTimecode { minutes: t.minutes, seconds: t.seconds, frames: t.frames }
    }
}

/// The total play time of the area from `slot` as stored (area TOC offset 64,
/// specification 3); `SbAreaInfo.playtime_frames` is its frame number.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `out` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_area_playtime_stored(
    disc: *const SbDisc,
    slot: u32,
    out: *mut SbTimecode,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?.get()?;
        let a = find_area(&d, slot)?;
        // SAFETY: forwarded contract.
        unsafe { write_out(out, SbTimecode::from(a.playtime)) }?;
        Ok(SbStatus::Ok)
    })
}

/// The start and duration of track `number` of the area from `slot` as stored in the area
/// TOC's `SACDTRL2` (specification 3, 3.7.2): before the clamping of 3.9.2, whatever their
/// range. `SbTrackInfo.start_frame` and `duration_frames` are their frame numbers after
/// clamping. Both outputs are required.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `start` and `duration` NULL or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_track_timecodes_stored(
    disc: *const SbDisc,
    slot: u32,
    number: u32,
    start: *mut SbTimecode,
    duration: *mut SbTimecode,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?.get()?;
        if start.is_null() || duration.is_null() {
            return Err(Failure::arg("NULL output pointer"));
        }
        let (s, l) = d.track_timecodes_stored(slot_from(slot)?, number)?;
        // SAFETY: forwarded contract (checked non-NULL above).
        unsafe { write_out(start, SbTimecode::from(s)) }?;
        // SAFETY: forwarded contract (checked non-NULL above).
        unsafe { write_out(duration, SbTimecode::from(l)) }?;
        Ok(SbStatus::Ok)
    })
}

/// More TOC fields of the area from `slot` (`SbAreaDetails`).
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `out` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_area_details(
    disc: *const SbDisc,
    slot: u32,
    out: *mut SbAreaDetails,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?.get()?;
        let details = area_details(find_area(&d, slot)?);
        // SAFETY: forwarded contract.
        unsafe { write_out(out, details) }?;
        Ok(SbStatus::Ok)
    })
}

/// The locale entry of text channel `index` of the area from `slot`, below
/// `min(SbAreaDetails.text_channel_count, 10)` (the reported area locale list,
/// specification 3, 3.5); `SB_STATUS_INVALID_ARGUMENT` for another index.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `out` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_area_text_channel(
    disc: *const SbDisc,
    slot: u32,
    index: u32,
    out: *mut SbTextChannel,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?.get()?;
        let channel = text_channel(&find_area(&d, slot)?.locales, index)?;
        // SAFETY: forwarded contract.
        unsafe { write_out(out, channel) }?;
        Ok(SbStatus::Ok)
    })
}

/// Information about track `number` (1-based) of the area from `slot`.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `out` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_track_info(
    disc: *const SbDisc,
    slot: u32,
    number: u32,
    out: *mut SbTrackInfo,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?.get()?;
        let area = find_area(&d, slot)?;
        let t = area.track(number).ok_or(Error::NoSuchTrack { slot: area.slot, track: number })?;
        // SAFETY: forwarded contract.
        unsafe { write_out(out, track_info(t)) }?;
        Ok(SbStatus::Ok)
    })
}

/// Copies a track string (`SbTrackString`).
///
/// # Safety
///
/// As `sb_disc_string`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_track_string(
    disc: *const SbDisc,
    slot: u32,
    number: u32,
    field: u32,
    buf: *mut c_char,
    cap: usize,
    len: *mut usize,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?.get()?;
        let area = find_area(&d, slot)?;
        let t = area.track(number).ok_or(Error::NoSuchTrack { slot: area.slot, track: number })?;
        let s = track_string(t, field)?;
        // SAFETY: forwarded contract.
        unsafe { copy_string(s, buf, cap, len) }
    })
}

fn warning_code(code: WarningCode) -> &'static CStr {
    match code {
        WarningCode::MasterTextMissing => c"master-text-missing",
        WarningCode::ManufacturerMissing => c"manufacturer-missing",
        WarningCode::AreaTocTooLarge => c"area-toc-too-large",
        WarningCode::AreaToc1Unreadable => c"area-toc1-unreadable",
        WarningCode::AreaToc2Unreadable => c"area-toc2-unreadable",
        WarningCode::AreaTocCopiesDiffer => c"area-toc-copies-differ",
        WarningCode::AreaTocUnreadable => c"area-toc-unreadable",
        WarningCode::AreaSignatureInvalid => c"area-signature-invalid",
        WarningCode::AreaTrackListMissing => c"area-track-list-missing",
        WarningCode::AreaTocMalformed => c"area-toc-malformed",
        WarningCode::AreaAudioRangeReversed => c"area-audio-range-reversed",
        WarningCode::TrackTextOffsetInvalid => c"track-text-offset-invalid",
        WarningCode::TrackTextTypeUnknown => c"track-text-type-unknown",
        WarningCode::TrackPastPlaytime => c"track-past-playtime",
        WarningCode::TrackLsnOutsideArea => c"track-lsn-outside-area",
        WarningCode::AudioSectorMalformed => c"audio-sector-malformed",
        WarningCode::MasterTocCopiesDiffer => c"master-toc-copies-differ",
        WarningCode::SampleFrequencyUnknown => c"sample-frequency-unknown",
        _ => c"unknown",
    }
}

fn sb_warning(w: &scarletbook::Warning) -> SbWarning {
    SbWarning {
        lsn: w.lsn.map_or(-1, i64::from),
        slot: w.slot.map_or(-1, |s| slot_index(s) as i32),
        track: w.track.map_or(0, u32::from),
        code: warning_code(w.code).as_ptr(),
    }
}

/// Warning `index` (0 to `warning_count - 1`) emitted while reading the TOC.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `out` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_warning(disc: *const SbDisc, index: u32, out: *mut SbWarning) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?.get()?;
        let w = d.warnings().get(index as usize).ok_or_else(|| Failure::arg("warning index out of range"))?;
        let info = sb_warning(w);
        // SAFETY: forwarded contract.
        unsafe { write_out(out, info) }?;
        Ok(SbStatus::Ok)
    })
}

/// Size of a logical sector's user data in bytes (specification 0, 0.3), the unit of
/// `sb_disc_read_sectors`.
pub const SB_SECTOR_SIZE: u32 = 2048;

const _: () = assert!(SB_SECTOR_SIZE as usize == scarletbook::SECTOR_SIZE);

/// Copies the user data of the logical sectors `[lsn, lsn + count)` into `buf`, 2048
/// bytes (`SB_SECTOR_SIZE`) per sector in order, whatever the image's stored sector size
/// (`SbDiscInfo.sector_size`): the data a 2048-byte-sector image holds at those sectors.
/// Works on every disc handle, clones and callback discs included; the disc's read
/// callbacks run during this call, and its log reports the read as it does other reads.
///
/// A read is whole or nothing (specification 1, 1.3): if any sector of the range is at or
/// past `SbDiscInfo.sectors`, the call returns `SB_STATUS_SECTOR_RANGE_INVALID` and
/// writes nothing; with `buf` NULL or `cap < count x 2048`, `SB_STATUS_BUFFER_TOO_SMALL`,
/// and writes nothing. `count` 0 reads nothing and returns `SB_STATUS_OK` for any `lsn`
/// up to `SbDiscInfo.sectors`. Nothing is ever written past `count x 2048` bytes of
/// `buf`; after `SB_STATUS_IO`, what those bytes hold is unspecified.
///
/// The image is read many sectors at a time, not sector by sector: a range of an image of
/// 2048-byte stored sectors in one read, of other images in reads of 64 stored sectors.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `buf` NULL or valid for `cap` bytes of writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_read_sectors(
    disc: *const SbDisc,
    lsn: u64,
    count: u32,
    buf: *mut u8,
    cap: usize,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let mut d = unsafe { disc_handle(disc) }?.get_mut()?;
        let image = d.image_mut();
        let need = (count as usize).checked_mul(scarletbook::SECTOR_SIZE);
        if !image.contains(lsn, u64::from(count)) {
            // As the image reports a read past its end.
            let n = image.sector_count();
            image.log().write(
                Level::Debug,
                format_args!("read of {count} sectors at LSN {lsn}: past the last sector ({n})"),
            );
            let msg = format!(
                "sectors {lsn}..{} are not all in the image's {} sectors",
                lsn.saturating_add(u64::from(count)),
                image.sector_count()
            );
            return Err(Failure(SbStatus::SectorRangeInvalid, msg));
        }
        let Some(need) = need.filter(|&n| n == 0 || (!buf.is_null() && n <= cap)) else {
            let msg = format!("{count} sectors need {} bytes", u64::from(count) * 2048);
            return Err(Failure(SbStatus::BufferTooSmall, msg));
        };
        if need == 0 {
            return Ok(SbStatus::Ok);
        }
        // SAFETY: non-NULL and valid for cap >= need bytes of writes by this function's
        // contract; the library holds no reference into the caller's memory.
        let out = unsafe { std::slice::from_raw_parts_mut(buf, need) };
        // In range (checked above), so a failure is an I/O error.
        if image.read_sectors(lsn, out)? {
            Ok(SbStatus::Ok)
        } else {
            Err(Failure(SbStatus::SectorRangeInvalid, format!("sectors from {lsn} are not all in the image")))
        }
    })
}

/// Whether track `number` of the area from `slot` produces audio: `SB_STATUS_OK`, or the
/// per-track outcome (`SB_STATUS_TRACK_EMPTY`, `SB_STATUS_CHANNEL_COUNT_UNSUPPORTED`,
/// `SB_STATUS_TRACK_EXCEEDS_CAPACITY`).
///
/// # Safety
///
/// `disc` must be NULL or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_track_check(disc: *const SbDisc, slot: u32, number: u32) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?.get()?;
        d.check_track(slot_from(slot)?, number)?;
        Ok(SbStatus::Ok)
    })
}

/// Opens the decoded audio of a track. Fails with the per-track outcome if the track has
/// no audio. On failure `*out` is set to NULL.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `out` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_track_audio_open(
    disc: *const SbDisc,
    slot: u32,
    number: u32,
    out: *mut *mut SbTrackAudio,
) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { clear_out(out) };
    guard(|| {
        // SAFETY: forwarded contract.
        let handle = unsafe { disc_handle(disc) }?;
        if out.is_null() {
            return Err(Failure::arg("NULL output pointer"));
        }
        let cursor = handle.get()?.track_audio_cursor(slot_from(slot)?, number)?;
        let audio = SbTrackAudio::fresh(cursor, handle.id);
        // SAFETY: non-NULL and valid for a write by this function's contract.
        unsafe { out.write(Box::into_raw(audio)) };
        Ok(SbStatus::Ok)
    })
}

/// Opens the decoded audio of track `number` of the area from `slot`, continuing `prev`
/// (an earlier track's or range's handle from the same disc or a clone of it) without rescanning the area
/// where the result is the same. Reading an area's tracks in increasing order this way
/// reads each audio sector once. With `prev` NULL this is `sb_track_audio_open`.
///
/// On success `prev` is consumed (freed) and `*out` receives the new handle. On failure
/// `prev` is left untouched (still to be freed by the caller) and `*out` is set to NULL,
/// with one exception: after `SB_STATUS_PANIC`, treat `prev` as consumed and neither use
/// nor free it (an internal error after ownership was taken would otherwise risk a double
/// free; the handle may leak). Check a track with `sb_track_check` first to keep `prev`
/// across tracks without audio.
///
/// Do not pass the address of the variable holding `prev` as `out`
/// (`sb_track_audio_open_next(d, h, slot, n, &h)`): `*out` is set to NULL before anything
/// else, so on failure the only pointer to `prev` would be lost. Use a second variable.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `prev` NULL or a live handle, not used again
/// after a successful call; `out` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_track_audio_open_next(
    disc: *const SbDisc,
    prev: *mut SbTrackAudio,
    slot: u32,
    number: u32,
    out: *mut *mut SbTrackAudio,
) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { open_next(disc, prev, slot, Target::Track(number), std::ptr::null_mut(), out) }
}

/// What `open_next` opens.
#[derive(Clone, Copy)]
enum Target {
    Track(u32),
    Range(u32, u32),
}

/// The shared body of `sb_track_audio_open_next` and `sb_range_audio_open_next`.
///
/// # Safety
///
/// As `sb_track_audio_open_next`.
unsafe fn open_next(
    disc: *const SbDisc,
    prev: *mut SbTrackAudio,
    slot: u32,
    target: Target,
    index: *mut SbSeekIndex,
    out: *mut *mut SbTrackAudio,
) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { clear_out(out) };
    guard(|| {
        // SAFETY: forwarded contract.
        let handle = unsafe { disc_handle(disc) }?;
        if out.is_null() {
            return Err(Failure::arg("NULL output pointer"));
        }
        let slot = slot_from(slot)?;
        let d = handle.get()?;
        // The index stays borrowed to the end: a callback that reaches it gets
        // SB_STATUS_INVALID_ARGUMENT.
        let mut index = if index.is_null() {
            None
        } else {
            // SAFETY: forwarded contract.
            let i = unsafe { index_handle(index) }?;
            Some(i.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?)
        };
        if prev.is_null() {
            let cursor = match (target, &index) {
                (Target::Track(n), _) => d.track_audio_cursor(slot, n)?,
                (Target::Range(first, end), None) => d.range_audio_cursor(slot, first, end)?,
                (Target::Range(first, end), Some(i)) => d.range_audio_cursor_indexed(slot, first, end, i)?,
            };
            let audio = SbTrackAudio::fresh(cursor, handle.id);
            // SAFETY: non-NULL and valid for a write by this function's contract.
            unsafe { out.write(Box::into_raw(audio)) };
            return Ok(SbStatus::Ok);
        }
        // Every check that can fail comes before `prev` is taken, so a failure leaves it
        // untouched; continuing into a checked track or range cannot fail.
        // SAFETY: forwarded contract.
        let p = unsafe { audio_handle(prev) }?;
        if p.disc_id != handle.id {
            return Err(Failure::arg("the previous track audio handle belongs to another disc"));
        }
        // `prev` stays borrowed while a log callback can run (the checks and the opening
        // are logged): a callback that reaches it then gets SB_STATUS_INVALID_ARGUMENT,
        // or a no-op from sb_track_audio_free, instead of freeing or reading it.
        let borrowed = p.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?;
        let next = match target {
            Target::Track(n) => d.track_continuation(slot, n)?,
            Target::Range(first, end) => d.range_continuation(slot, first, end)?,
        };
        // Decided once, so that the line logged says what is done.
        let opening = match &index {
            Some(i) => {
                next.check_index(i)?;
                next.log_audio_indexed(&borrowed, i)
            }
            None => next.log_audio(&borrowed),
        };
        drop(borrowed);
        // From here to the end, nothing logs: no callback runs while `prev` is consumed.
        // SAFETY: a live handle created by Box::into_raw and not in use (no borrow is
        // active, and no callback runs before it is freed); the caller does not use it
        // again after this successful call.
        let prev_box = unsafe { Box::from_raw(prev) };
        // A position held for a too small buffer (sb_track_audio_read_with_stored) is
        // dropped with the frames held ahead: the cursor stands after them, so a range
        // that starts at or before it scans afresh.
        let prev_cursor = prev_box.inner.into_inner();
        let cursor = match index.as_deref_mut() {
            Some(i) => next.audio_indexed_as(prev_cursor, i, opening),
            None => next.audio_as(prev_cursor, opening),
        };
        let audio = Box::new(SbTrackAudio {
            inner: RefCell::new(cursor),
            disc_id: handle.id,
            held: RefCell::new(None),
        });
        // SAFETY: non-NULL and valid for a write by this function's contract.
        unsafe { out.write(Box::into_raw(audio)) };
        Ok(SbStatus::Ok)
    })
}

/// Track `number`'s track with pause (specification 4, 4.7.1 item 5): `[a, b)` with
/// `a = 0` for track 1 and the track's own start otherwise, and `b` the larger of the
/// track's own end and the start of the track with the next number (the play time for
/// the last track). With tracks out of order or overlapping these ranges may overlap.
/// Pass `*first` and `*end` to `sb_range_audio_open`. Both pointers are required.
/// Fails with `SB_STATUS_NO_SUCH_AREA` or `SB_STATUS_NO_SUCH_TRACK`.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `first` and `end` NULL or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_track_range_with_pause(
    disc: *const SbDisc,
    slot: u32,
    number: u32,
    first: *mut u32,
    end: *mut u32,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?.get()?;
        if first.is_null() || end.is_null() {
            return Err(Failure::arg("NULL output pointer"));
        }
        let (a, b) = d.track_range_with_pause(slot_from(slot)?, number)?;
        // SAFETY: forwarded contract (checked non-NULL above).
        unsafe { write_out(first, a) }?;
        // SAFETY: forwarded contract (checked non-NULL above).
        unsafe { write_out(end, b) }?;
        Ok(SbStatus::Ok)
    })
}

/// Whether the frame range `[first, end)` of the area from `slot` produces audio:
/// `SB_STATUS_OK`, or `SB_STATUS_FRAME_RANGE_INVALID` unless `first <= end <= P` (`P`
/// the area's play time, `SbAreaInfo.playtime_frames`; a caller error), or the outcomes
/// of specification 4, 4.8 applied to the range as to a track (4.7.1 item 4):
/// `SB_STATUS_CHANNEL_COUNT_UNSUPPORTED`, `SB_STATUS_TRACK_EMPTY` when `first == end`,
/// `SB_STATUS_TRACK_EXCEEDS_CAPACITY`.
///
/// # Safety
///
/// `disc` must be NULL or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_range_check(disc: *const SbDisc, slot: u32, first: u32, end: u32) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?.get()?;
        d.check_range(slot_from(slot)?, first, end)?;
        Ok(SbStatus::Ok)
    })
}

/// Opens the decoded audio of the frame range `[first, end)` of the area from `slot`:
/// the timeline positions `first` to `end - 1`, numbered, filled and counted exactly as a
/// track's. `[0, P)`, with `P` the area's play time, is the whole area with its pauses.
/// The handle is read, queried and freed with the `sb_track_audio_*` functions. Fails as
/// `sb_range_check` does; on failure `*out` is set to NULL.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `out` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_range_audio_open(
    disc: *const SbDisc,
    slot: u32,
    first: u32,
    end: u32,
    out: *mut *mut SbTrackAudio,
) -> SbStatus {
    // SAFETY: forwarded contract (a NULL `prev` opens afresh).
    unsafe { sb_range_audio_open_next(disc, std::ptr::null_mut(), slot, first, end, out) }
}

/// Opens the decoded audio of the frame range `[first, end)` of the area from `slot`,
/// continuing `prev` (an earlier track's or range's handle from the same disc or a clone of it) as
/// `sb_track_audio_open_next` does, with the same ownership rules. With `prev` NULL this
/// is `sb_range_audio_open`.
///
/// # Safety
///
/// As `sb_track_audio_open_next`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_range_audio_open_next(
    disc: *const SbDisc,
    prev: *mut SbTrackAudio,
    slot: u32,
    first: u32,
    end: u32,
    out: *mut *mut SbTrackAudio,
) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { open_next(disc, prev, slot, Target::Range(first, end), std::ptr::null_mut(), out) }
}

/// The channel count of a track's audio (1 to 6), or 0 for NULL or a handle in use.
///
/// # Safety
///
/// `audio` must be NULL or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_track_audio_channels(audio: *const SbTrackAudio) -> u32 {
    // SAFETY: forwarded contract.
    let Ok(a) = (unsafe { audio_handle(audio) }) else { return 0 };
    a.inner.try_borrow().map_or(0, |c| u32::from(c.channels()))
}

/// Decodes the next frame of a track into `buf` (interleaved, channels x 4704 bytes).
/// Returns `SB_STATUS_END` after the last frame. `disc` must be the live disc the track
/// was opened from or a clone of it (`sb_disc_clone`). The disc's read callbacks, if any,
/// run during this call.
///
/// # Safety
///
/// `audio` and `disc` must be NULL or live handles; `buf` NULL or valid for `cap` bytes of
/// writes; `info` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_track_audio_read(
    audio: *const SbTrackAudio,
    disc: *const SbDisc,
    buf: *mut u8,
    cap: usize,
    info: *mut SbFrameInfo,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let a = unsafe { audio_handle(audio) }?;
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?;
        if a.disc_id != d.id {
            return Err(Failure::arg("the track audio handle belongs to another disc"));
        }
        let mut cursor = a.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?;
        let size = usize::from(cursor.channels()) * scarletbook::audio::FRAME_BYTES_PER_CHANNEL;
        if buf.is_null() || cap < size {
            return Err(Failure(SbStatus::BufferTooSmall, format!("a frame needs {size} bytes")));
        }
        let mut held = a.held.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?;
        let frame = match held.take() {
            // Held by sb_track_audio_read_with_stored: its stored content is dropped.
            Some((frame, _)) => frame,
            None => {
                let mut disc = d.get_mut()?;
                let frame = cursor.next_frame(disc.image_mut());
                // The disc's seek index (`sb_disc_seek_index_enable`), shared with its
                // clones, takes what the read recorded: a test when there is nothing.
                disc.absorb_seek_checkpoints(&mut *cursor);
                let Some(frame) = frame else {
                    return Ok(SbStatus::End);
                };
                frame?
            }
        };
        // SAFETY: forwarded contract (buf checked above for `size` bytes).
        unsafe { write_decoded(&frame, size, buf, info) };
        Ok(SbStatus::Ok)
    })
}

/// Copies a decoded frame into `buf` and describes it in `*info` (if not NULL).
///
/// # Safety
///
/// `buf` must be valid for `size` bytes of writes, `size` the frame size of the handle's
/// area (channels x 4704); `info` NULL or valid for a write.
unsafe fn write_decoded(frame: &AudioFrame, size: usize, buf: *mut u8, info: *mut SbFrameInfo) {
    // SAFETY: forwarded contract.
    unsafe { copy_decoded(frame, size, buf) };
    if !info.is_null() {
        // SAFETY: non-NULL and valid for a write by this function's contract.
        unsafe { info.write(decoded_info(frame, size)) };
    }
}

/// Copies a decoded frame into `buf`.
///
/// # Safety
///
/// `buf` must be valid for `size` bytes of writes.
unsafe fn copy_decoded(frame: &AudioFrame, size: usize, buf: *mut u8) {
    let n = frame.data.len().min(size);
    // SAFETY: buf is valid for size >= n bytes of writes; frame.data is owned by this
    // library, so the regions do not overlap.
    unsafe { std::ptr::copy_nonoverlapping(frame.data.as_ptr(), buf, n) };
}

/// The `SbFrameInfo` of a decoded frame copied into a buffer of `size` bytes.
fn decoded_info(frame: &AudioFrame, size: usize) -> SbFrameInfo {
    let (status, rule) = match frame.status {
        FrameStatus::Dsd => (SbFrameStatus::Dsd, 0),
        FrameStatus::Decoded => (SbFrameStatus::Decoded, 0),
        FrameStatus::Missing => (SbFrameStatus::Missing, 0),
        FrameStatus::Invalid(e) => (SbFrameStatus::Invalid, u32::from(e.rule())),
    };
    // At most size = channels x 4704, so it fits.
    let n = frame.data.len().min(size);
    SbFrameInfo { number: frame.number, status: status as u32, dst_rule: rule, size: n as u32 }
}

/// Makes a track or range audio handle keep each position's stored content next to its
/// decoding, for `sb_track_audio_read_with_stored`: from one read of the area's sectors,
/// what `sb_raw_frames_read` gives for each position and what `sb_track_audio_read` gives,
/// decoded where the handle decodes (`sb_track_audio_set_pool`) with the same frames held
/// ahead. Call it before the first read (`SB_STATUS_INVALID_ARGUMENT` afterwards); handles
/// continued from it (`sb_track_audio_open_next`, `sb_range_audio_open_next`) keep it.
/// Each DST frame's bytes (at most `SB_MAX_FRAME_BYTES`) are then kept until the position
/// is read.
///
/// # Safety
///
/// `audio` must be NULL or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_track_audio_keep_stored(audio: *const SbTrackAudio) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let a = unsafe { audio_handle(audio) }?;
        a.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?.keep_stored()?;
        Ok(SbStatus::Ok)
    })
}

/// Reads the next position of a handle that keeps stored content
/// (`sb_track_audio_keep_stored`): its decoded audio into `buf` and `*info`, exactly as
/// `sb_track_audio_read` writes them, and its stored content into `raw_buf` and
/// `*raw_info`, exactly as `sb_raw_frames_read` writes them for the same position (with
/// `dst_equivalent` 1, as `sb_raw_frames_read_dst_equivalent` does: the position's DST
/// equivalent). Returns `SB_STATUS_END` after the last position.
///
/// `buf` needs channels x 4704 bytes, `raw_buf` the position's size (`SB_MAX_FRAME_BYTES`
/// always suffices; a missing position has none, unless `dst_equivalent`). If `raw_buf`
/// is too small, the call writes `*info` and `*raw_info` (with the size needed) but
/// neither buffer, returns `SB_STATUS_BUFFER_TOO_SMALL`, and keeps the position for the
/// next call (or for `sb_track_audio_read`, which then drops its stored content). A handle
/// continued with a position held (`sb_track_audio_open_next`, `sb_range_audio_open_next`)
/// drops it, as `sb_raw_frames_open_next` does: the reading stands after it, so a range
/// that starts at or before it scans afresh and returns it again. The handle's other calls
/// behave as for any track audio handle; `sb_track_audio_read` may be mixed with this one.
///
/// # Safety
///
/// `audio` and `disc` must be NULL or live handles; `buf` NULL or valid for `cap` bytes of
/// writes; `raw_buf` NULL or valid for `raw_cap` bytes of writes, not overlapping `buf`;
/// `info` and `raw_info` NULL or valid for a write.
#[allow(clippy::too_many_arguments, reason = "the two buffers of sb_track_audio_read and sb_raw_frames_read")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_track_audio_read_with_stored(
    audio: *const SbTrackAudio,
    disc: *const SbDisc,
    buf: *mut u8,
    cap: usize,
    info: *mut SbFrameInfo,
    raw_buf: *mut u8,
    raw_cap: usize,
    raw_info: *mut SbRawFrameInfo,
    dst_equivalent: u8,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let a = unsafe { audio_handle(audio) }?;
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?;
        if a.disc_id != d.id {
            return Err(Failure::arg("the track audio handle belongs to another disc"));
        }
        if dst_equivalent > 1 {
            return Err(Failure::arg("dst_equivalent must be 0 or 1"));
        }
        let mut cursor = a.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?;
        if !cursor.keeps_stored() {
            return Err(Failure::arg("the handle does not keep stored content (sb_track_audio_keep_stored)"));
        }
        let size = usize::from(cursor.channels()) * scarletbook::audio::FRAME_BYTES_PER_CHANNEL;
        if buf.is_null() || cap < size {
            return Err(Failure(SbStatus::BufferTooSmall, format!("a frame needs {size} bytes")));
        }
        let mut held = a.held.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?;
        let (frame, stored) = match held.take() {
            Some(pair) => pair,
            None => {
                let mut disc = d.get_mut()?;
                let pair = cursor.next_frame_with_stored(disc.image_mut());
                // As in sb_track_audio_read.
                disc.absorb_seek_checkpoints(&mut *cursor);
                let Some(pair) = pair else {
                    return Ok(SbStatus::End);
                };
                pair?
            }
        };
        // Stored content and DST equivalents: specification 4, 4.6.1 items 1 to 4.
        let kind = match &stored {
            Stored::Dst(_) => SbFrameKind::Dst,
            Stored::Plain => SbFrameKind::Dsd,
            Stored::Missing => SbFrameKind::Missing,
        };
        let bytes: std::borrow::Cow<'_, [u8]> = match (&stored, dst_equivalent == 1) {
            (Stored::Dst(b), _) => std::borrow::Cow::Borrowed(b),
            (_, false) => std::borrow::Cow::Borrowed(stored.bytes(&frame)),
            (_, true) => std::borrow::Cow::Owned(cursor.dst_equivalent(&stored.content(&frame)).into_owned()),
        };
        let raw_size = bytes.len();
        if !info.is_null() {
            // SAFETY: non-NULL and valid for a write by this function's contract.
            unsafe { info.write(decoded_info(&frame, size)) };
        }
        if !raw_info.is_null() {
            // At most SB_MAX_FRAME_BYTES (a stored frame or its DST equivalent), so it fits.
            let ri = SbRawFrameInfo { number: frame.number, kind: kind as u32, size: raw_size as u32 };
            // SAFETY: non-NULL and valid for a write by this function's contract.
            unsafe { raw_info.write(ri) };
        }
        if raw_size > 0 && (raw_buf.is_null() || raw_cap < raw_size) {
            let msg = format!("frame {} needs {raw_size} bytes of stored content", frame.number);
            drop(bytes);
            *held = Some((frame, stored));
            return Err(Failure(SbStatus::BufferTooSmall, msg));
        }
        if raw_size > 0 {
            // SAFETY: raw_buf is valid for raw_cap >= raw_size bytes of writes; `bytes` is
            // owned by this library, so the regions do not overlap.
            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), raw_buf, raw_size) };
        }
        // SAFETY: buf is valid for cap >= size bytes of writes (checked above).
        unsafe { copy_decoded(&frame, size, buf) };
        Ok(SbStatus::Ok)
    })
}

/// The missing and invalid DST frame counts of the frames returned so far.
///
/// # Safety
///
/// `audio` must be NULL or a live handle; `missing` and `invalid` NULL or valid for writes
/// (both are required; nothing is written unless both are given).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_track_audio_counts(
    audio: *const SbTrackAudio,
    missing: *mut u64,
    invalid: *mut u64,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let a = unsafe { audio_handle(audio) }?;
        if missing.is_null() || invalid.is_null() {
            return Err(Failure::arg("NULL output pointer"));
        }
        let cursor = a.inner.try_borrow().map_err(|_| Failure::arg(REENTRANT))?;
        // A held position (sb_track_audio_read_with_stored) has been counted by the cursor
        // but not yet returned.
        let held = a.held.try_borrow().map_err(|_| Failure::arg(REENTRANT))?;
        let status = held.as_ref().map(|(f, _)| f.status);
        let (held_missing, held_invalid) = match status {
            Some(FrameStatus::Missing) => (1, 0),
            Some(FrameStatus::Invalid(_)) => (0, 1),
            _ => (0, 0),
        };
        // SAFETY: forwarded contract (checked non-NULL above).
        unsafe { write_out(missing, cursor.missing_frames() - held_missing) }?;
        // SAFETY: forwarded contract (checked non-NULL above).
        unsafe { write_out(invalid, cursor.invalid_dst_frames() - held_invalid) }?;
        Ok(SbStatus::Ok)
    })
}

/// The number of malformed audio sectors processed so far by this handle's scan of its
/// area. The scan starts at the area's first audio sector, not at the track; a handle from
/// `sb_track_audio_open_next` keeps counting from where the earlier tracks left off.
///
/// # Safety
///
/// `audio` must be NULL or a live handle; `count` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_track_audio_malformed_sectors(
    audio: *const SbTrackAudio,
    count: *mut u64,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let a = unsafe { audio_handle(audio) }?;
        let cursor = a.inner.try_borrow().map_err(|_| Failure::arg(REENTRANT))?;
        // SAFETY: forwarded contract.
        unsafe { write_out(count, cursor.malformed_sectors()) }?;
        Ok(SbStatus::Ok)
    })
}

/// `audio-sector-malformed` warning `index` of this handle's scan of its area so far, with
/// its slot and LSN (specification 4, 4.4.5): per area, not per track, and each sector
/// once (sectors of earlier tracks of a continued handle are not reported again). At
/// most 4096 are kept per scan; `sb_track_audio_malformed_sectors` counts them all.
/// Returns `SB_STATUS_INVALID_ARGUMENT` past the last one.
///
/// # Safety
///
/// `audio` must be NULL or a live handle; `out` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_track_audio_warning(
    audio: *const SbTrackAudio,
    index: u32,
    out: *mut SbWarning,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let a = unsafe { audio_handle(audio) }?;
        let cursor = a.inner.try_borrow().map_err(|_| Failure::arg(REENTRANT))?;
        let w = cursor
            .warnings()
            .get(index as usize)
            .ok_or_else(|| Failure::arg("warning index out of range"))?;
        // SAFETY: forwarded contract.
        unsafe { write_out(out, sb_warning(w)) }?;
        Ok(SbStatus::Ok)
    })
}

/// Frees a track audio handle. NULL is ignored, and so is a handle whose read is running
/// (the call is then a no-op).
///
/// # Safety
///
/// `audio` must be NULL or a live handle, freed only once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_track_audio_free(audio: *mut SbTrackAudio) {
    let _ = guard(|| {
        // SAFETY: forwarded contract.
        let handle = unsafe { audio_handle(audio) }?;
        if handle.inner.try_borrow_mut().is_err() {
            return Err(Failure::arg(REENTRANT));
        }
        // SAFETY: a live handle created by Box::into_raw and not in use, freed only once.
        drop(unsafe { Box::from_raw(audio) });
        Ok(SbStatus::Ok)
    });
}

/// Most threads of a decoding pool (`sb_pool_new`). A handle keeps at most
/// `16 x threads` frames ahead, and never more than 512: fewer than 2 per thread beyond
/// this many threads, which would mostly stay idle.
pub const SB_MAX_POOL_THREADS: u32 = 256;

/// Starts a pool of `threads` decoding threads for `sb_track_audio_set_pool`: 1 to
/// `SB_MAX_POOL_THREADS`, or 0 for one per logical CPU (at most `SB_MAX_POOL_THREADS`,
/// whatever `RAYON_NUM_THREADS` says). The pool belongs to the caller: no global pool is
/// created or used. On failure `*out` is set to NULL: `SB_STATUS_INVALID_ARGUMENT` for
/// more than `SB_MAX_POOL_THREADS` threads, `SB_STATUS_THREADS_UNAVAILABLE` when the
/// operating system cannot start them (those already started then exit).
///
/// A pool may be used by several threads at once and shared by the track audio handles of
/// any number of discs. It is reference-counted: each handle given it holds a reference,
/// so `sb_pool_free` may come before or after `sb_track_audio_free`. Its threads exit,
/// without being waited for, once the pool and every handle given it are freed (or given
/// another pool), after finishing the frames already sent to them.
///
/// # Safety
///
/// `out` must be NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_pool_new(threads: u32, out: *mut *mut SbPool) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { clear_out(out) };
    guard(|| {
        if out.is_null() {
            return Err(Failure::arg("NULL output pointer"));
        }
        if threads > SB_MAX_POOL_THREADS {
            return Err(Failure::arg(&format!(
                "a pool has at most {SB_MAX_POOL_THREADS} threads, not {threads}"
            )));
        }
        let threads = match threads {
            0 => std::thread::available_parallelism()
                .map_or(1, |n| u32::try_from(n.get()).unwrap_or(u32::MAX))
                .min(SB_MAX_POOL_THREADS),
            n => n,
        };
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads as usize)
            .thread_name(|i| format!("scarletbook-{i}"))
            .build()
            .map_err(|e| {
                Failure(SbStatus::ThreadsUnavailable, format!("cannot start {threads} threads: {e}"))
            })?;
        let pool = Box::new(SbPool { pool: Arc::new(pool), threads });
        // SAFETY: non-NULL and valid for a write by this function's contract.
        unsafe { out.write(Box::into_raw(pool)) };
        Ok(SbStatus::Ok)
    })
}

/// The number of threads of a pool, or 0 for NULL.
///
/// # Safety
///
/// `pool` must be NULL or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_pool_threads(pool: *const SbPool) -> u32 {
    // SAFETY: NULL or a live handle by this function's contract; only shared references
    // to it are ever made.
    unsafe { pool.as_ref() }.map_or(0, |p| p.threads)
}

/// Frees a pool. NULL is ignored. Track audio handles given it keep its threads running
/// until they are freed or given another pool; the call does not wait for the threads.
///
/// # Safety
///
/// `pool` must be NULL or a live handle, freed only once, and not in use by another call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_pool_free(pool: *mut SbPool) {
    let _ = guard(|| {
        if !pool.is_null() {
            // SAFETY: a live handle created by Box::into_raw, not in use, and freed only
            // once (this function's contract).
            drop(unsafe { Box::from_raw(pool) });
        }
        Ok(SbStatus::Ok)
    });
}

/// Makes a track or range audio handle decode its DST frames on `pool` from the next
/// `sb_track_audio_read` on or, with `pool` NULL, where handles decode by default: on the
/// calling thread (in a library built with the `parallel` feature, on rayon's global
/// pool). Handles continued from it by `sb_track_audio_open_next` and
/// `sb_range_audio_open_next` keep the choice. The handle holds a reference to the pool
/// (see `sb_pool_new`).
///
/// The reading stays on the calling thread: `sb_track_audio_read` reads the disc (and
/// runs its read callbacks) and returns the frames there, in order, while the pool's
/// threads decode DST frames ahead of it. The disc's log callback is then also invoked
/// from the pool's threads (see `sb_disc_set_log`). The frames returned, their statuses
/// and the counts are exactly those of decoding on the calling thread, whatever the pool
/// and whenever it is set or changed (frames already sent to an earlier pool finish
/// there). A pool of one thread decodes on the calling thread too.
///
/// Memory stays bounded whatever the image: a handle holds at most `16 x threads` frame
/// positions ahead of the caller, and never more than 512, each at most one stored frame
/// (`SB_MAX_FRAME_BYTES`) and one decoded frame (channels x 4704 bytes): about 48 MB at
/// most, about 6.8 MB for 2 channels and 20 MB for 6 with typical DST frames, plus its read
/// buffer: 64 KiB, or up to 1 MiB, kept for the handle's life, once it has made a read sized
/// to a provisional range. Each of the pool's threads also keeps decoder tables of up to
/// about 3.4 MB.
///
/// Returns `SB_STATUS_INVALID_ARGUMENT` when the handle is in use by a callback.
///
/// # Safety
///
/// `audio` and `pool` must be NULL or live handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_track_audio_set_pool(
    audio: *const SbTrackAudio,
    pool: *const SbPool,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let a = unsafe { audio_handle(audio) }?;
        // SAFETY: NULL or a live handle by this function's contract; only shared references
        // to it are ever made.
        let pool = unsafe { pool.as_ref() };
        let mut cursor = a.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?;
        cursor.set_decode_on(match pool {
            Some(p) => DecodeOn::Pool(Arc::clone(&p.pool)),
            None => SbTrackAudio::default_decode_on(),
        });
        Ok(SbStatus::Ok)
    })
}

/// Opens the frames of the frame range `[first, end)` of the area from `slot` without
/// decoding them (for example to copy DST frames unchanged into a DSDIFF file): for each
/// position from `first` to `end - 1`, its number, its kind and its bytes as stored. The
/// positions, missing frames and malformed sectors are exactly those of
/// `sb_range_audio_open` on the same range; decoding a DST frame's bytes with
/// `sb_dst_decode` gives that position's decoded audio. A track's frames are the range
/// `[start_frame, start_frame + duration_frames)` of its `SbTrackInfo`; `[0, P)`, with
/// `P` the area's play time, is the whole area. Fails as `sb_range_check` does; on
/// failure `*out` is set to NULL.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `out` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_raw_frames_open(
    disc: *const SbDisc,
    slot: u32,
    first: u32,
    end: u32,
    out: *mut *mut SbRawFrames,
) -> SbStatus {
    // SAFETY: forwarded contract (a NULL `prev` opens afresh).
    unsafe { sb_raw_frames_open_next(disc, std::ptr::null_mut(), slot, first, end, out) }
}

/// Opens the frames of the frame range `[first, end)` of the area from `slot` without
/// decoding them, continuing `prev` (an earlier range's raw frames handle from the same
/// disc) without rescanning the area where the result is the same: reading an area's
/// ranges in increasing order this way reads each audio sector once. The ownership rules
/// are those of `sb_track_audio_open_next`: on success `prev` is consumed, on failure it
/// is left untouched (except after `SB_STATUS_PANIC`). With `prev` NULL this is
/// `sb_raw_frames_open`.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `prev` NULL or a live handle, not used again
/// after a successful call; `out` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_raw_frames_open_next(
    disc: *const SbDisc,
    prev: *mut SbRawFrames,
    slot: u32,
    first: u32,
    end: u32,
    out: *mut *mut SbRawFrames,
) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { raw_open_next(disc, prev, slot, (first, end), std::ptr::null_mut(), out) }
}

/// The shared body of `sb_raw_frames_open_next` and `sb_raw_frames_open_next_indexed`.
///
/// # Safety
///
/// As `sb_raw_frames_open_next`, and `index` NULL or a live handle.
unsafe fn raw_open_next(
    disc: *const SbDisc,
    prev: *mut SbRawFrames,
    slot: u32,
    (first, end): (u32, u32),
    index: *mut SbSeekIndex,
    out: *mut *mut SbRawFrames,
) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { clear_out(out) };
    guard(|| {
        // SAFETY: forwarded contract.
        let handle = unsafe { disc_handle(disc) }?;
        if out.is_null() {
            return Err(Failure::arg("NULL output pointer"));
        }
        let slot = slot_from(slot)?;
        let d = handle.get()?;
        // Borrowed to the end, as in `open_next`.
        let mut index = if index.is_null() {
            None
        } else {
            // SAFETY: forwarded contract.
            let i = unsafe { index_handle(index) }?;
            Some(i.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?)
        };
        let cursor = if prev.is_null() {
            match &index {
                None => d.range_frames_cursor(slot, first, end)?,
                Some(i) => d.range_frames_cursor_indexed(slot, first, end, i)?,
            }
        } else {
            // Every check that can fail comes before `prev` is taken, so a failure leaves
            // it untouched; continuing into a checked range cannot fail.
            // SAFETY: forwarded contract.
            let p = unsafe { raw_handle(prev) }?;
            if p.disc_id != handle.id {
                return Err(Failure::arg("the previous raw frames handle belongs to another disc"));
            }
            // `prev` stays borrowed while a log callback can run (the check and the opening
            // are logged): a callback that reaches it then gets SB_STATUS_INVALID_ARGUMENT,
            // or a no-op from sb_raw_frames_free, instead of freeing or reading it.
            let borrowed = p.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?;
            let next = d.range_continuation(slot, first, end)?;
            if let Some(i) = &index {
                next.check_index(i)?;
            }
            // Decided once, so that the line logged says what is done.
            let opening = match (&borrowed.source, &index) {
                (Source::Range(c), Some(i)) => Some(next.log_frames_indexed(c, i)),
                (Source::Range(c), None) => Some(next.log_frames(c)),
                (Source::List(..), _) => {
                    next.log_fresh_frames();
                    None
                }
            };
            // A stored frame list records no checkpoints: after one, a range starts from
            // the index, made here, before `prev` is taken (it does not log or fail).
            let from_index = match (&borrowed.source, &index) {
                (Source::List(..), Some(i)) => Some(TrackCursor::for_range_indexed(
                    find_area(&d, slot_index(slot))?,
                    first,
                    end,
                    d.toc().sectors,
                    i,
                )?),
                _ => None,
            };
            drop(borrowed);
            // From here to the end, nothing logs: no callback runs while `prev` is consumed.
            // SAFETY: a live handle created by Box::into_raw and not in use (no borrow is
            // active, and no callback runs before it is freed); the caller does not use it
            // again after this successful call.
            let prev_box = unsafe { Box::from_raw(prev) };
            // A frame held for a too small buffer is dropped: the cursor stands after it,
            // so a range that starts at or before it scans afresh. A stored frame list's
            // scan is not a range's: the range then starts afresh.
            match (prev_box.inner.into_inner().source, index.as_deref_mut(), opening) {
                (Source::Range(c), Some(i), Some(opening)) => next.frames_indexed_as(c, i, opening),
                (Source::Range(c), None, Some(opening)) => next.frames_as(c, opening),
                (Source::Range(c), Some(i), None) => next.frames_indexed_unlogged(c, i),
                (Source::Range(c), None, None) => next.frames_unlogged(c),
                (Source::List(..), _, _) => match from_index {
                    Some(cursor) => cursor,
                    None => next.fresh_frames(),
                },
            }
        };
        let raw = Box::new(SbRawFrames {
            inner: RefCell::new(RawFrames { source: Source::Range(cursor), held: None }),
            disc_id: handle.id,
        });
        // SAFETY: non-NULL and valid for a write by this function's contract.
        unsafe { out.write(Box::into_raw(raw)) };
        Ok(SbStatus::Ok)
    })
}

/// Opens the **stored frame list** of the area from `slot` (specification 4, 4.6.1 item
/// 2): every accepted frame of the area in frame number order, including frames numbered
/// at or past the play time, read with `sb_raw_frames_read` (each as stored, never
/// `SB_FRAME_KIND_MISSING`: positions not listed are missing). Every audio sector is
/// processed. `sb_raw_frames_missing` gives 0 and `sb_raw_frames_read_dst_equivalent`
/// refuses: DST equivalents are written only for tracks and ranges (4.6.1 item 4). Fails
/// with `SB_STATUS_NO_SUCH_AREA` or `SB_STATUS_CHANNEL_COUNT_UNSUPPORTED` (4.6.1 item 1);
/// on failure `*out` is set to NULL. The handle may be passed as `prev` to
/// `sb_raw_frames_open_next`, which then opens its range afresh.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `out` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_stored_frame_list_open(
    disc: *const SbDisc,
    slot: u32,
    out: *mut *mut SbRawFrames,
) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { clear_out(out) };
    guard(|| {
        // SAFETY: forwarded contract.
        let handle = unsafe { disc_handle(disc) }?;
        if out.is_null() {
            return Err(Failure::arg("NULL output pointer"));
        }
        let d = handle.get()?;
        let slot = slot_from(slot)?;
        let cursor = d.stored_frame_list_cursor(slot)?;
        let channels = d.toc().area(slot).map_or(0, |a| a.channel_count);
        let raw = Box::new(SbRawFrames {
            inner: RefCell::new(RawFrames { source: Source::List(cursor, channels), held: None }),
            disc_id: handle.id,
        });
        // SAFETY: non-NULL and valid for a write by this function's contract.
        unsafe { out.write(Box::into_raw(raw)) };
        Ok(SbStatus::Ok)
    })
}

/// The channel count of a raw frames handle's area (1 to 6), or 0 for NULL or a handle
/// in use.
///
/// # Safety
///
/// `raw` must be NULL or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_raw_frames_channels(raw: *const SbRawFrames) -> u32 {
    // SAFETY: forwarded contract.
    let Ok(r) = (unsafe { raw_handle(raw) }) else { return 0 };
    r.inner.try_borrow().map_or(0, |r| u32::from(r.channels()))
}

/// Copies the next position's frame, as stored, into `buf` and describes it in `*info`:
/// its number, its `SbFrameKind` and its size (`channels x 4704` for a plain frame, the
/// DST frame's length for a DST frame, 0 for a missing position, never more than
/// `SB_MAX_FRAME_BYTES`). Returns `SB_STATUS_END` after the last position. `disc` must be
/// the live disc the handle was opened from or a clone of it; its read callbacks, if any,
/// run during this call. Invalid DST frames are not reported: validity is known only by decoding
/// (`sb_dst_decode`).
///
/// The kind is fixed by the DST flag of the sector the frame starts in (specification 4,
/// 4.3.1). A frame of a DST area stored as an uncoded DST frame (the byte 0x00, a plain DST
/// frame header, then `channels x 4704` bytes; specification 5, 5.2.1) is
/// `SB_FRAME_KIND_DST`, returned with its 0x00 prefix as stored; `SB_FRAME_KIND_DSD` is a
/// frame of a plain sector, without a prefix.
///
/// On any failure other than `SB_STATUS_BUFFER_TOO_SMALL` (an I/O error, a handle in use,
/// a handle of a disc that is not this one or a clone of it) and on `SB_STATUS_END`,
/// `*info` is not written.
///
/// If `buf` is NULL or `cap` is below the frame's size, nothing is copied, `*info` (if not
/// NULL) still receives the number, kind and needed size, and the call returns
/// `SB_STATUS_BUFFER_TOO_SMALL`: the same frame is returned by the next call. A buffer of
/// `SB_MAX_FRAME_BYTES` bytes always suffices; a missing position needs none.
///
/// # Safety
///
/// `raw` and `disc` must be NULL or live handles; `buf` NULL or valid for `cap` bytes of
/// writes; `info` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_raw_frames_read(
    raw: *const SbRawFrames,
    disc: *const SbDisc,
    buf: *mut u8,
    cap: usize,
    info: *mut SbRawFrameInfo,
) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { raw_read(raw, disc, buf, cap, info, false) }
}

/// As `sb_raw_frames_read`, but copies the position's **DST equivalent** (specification 4,
/// 4.6.1 item 3): a DST frame that decodes to exactly the position's audio. For a DST
/// frame its stored bytes; for a plain frame the byte 0x00 followed by its
/// `channels x 4704` bytes; for a missing position 0x00 followed by `channels x 4704`
/// bytes of 0x69. `*info` gives the stored kind and the equivalent's size (at most
/// `SB_MAX_FRAME_BYTES`, so a buffer of that size always suffices; every position needs
/// one). An uncoded DST frame (`SB_FRAME_KIND_DST` with its 0x00 prefix) is already its
/// own equivalent and is passed through unchanged, like every DST frame. Positions may be
/// read with either function in any mix. `*info` is written as for `sb_raw_frames_read`;
/// a stored frame list handle gets `SB_STATUS_INVALID_ARGUMENT`.
///
/// # Safety
///
/// As `sb_raw_frames_read`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_raw_frames_read_dst_equivalent(
    raw: *const SbRawFrames,
    disc: *const SbDisc,
    buf: *mut u8,
    cap: usize,
    info: *mut SbRawFrameInfo,
) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { raw_read(raw, disc, buf, cap, info, true) }
}

/// The body of `sb_raw_frames_read` and `sb_raw_frames_read_dst_equivalent`.
///
/// # Safety
///
/// As `sb_raw_frames_read`.
unsafe fn raw_read(
    raw: *const SbRawFrames,
    disc: *const SbDisc,
    buf: *mut u8,
    cap: usize,
    info: *mut SbRawFrameInfo,
    equivalent: bool,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let r = unsafe { raw_handle(raw) }?;
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?;
        if r.disc_id != d.id {
            return Err(Failure::arg("the raw frames handle belongs to another disc"));
        }
        let mut state = r.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?;
        if equivalent && matches!(state.source, Source::List(..)) {
            return Err(Failure::arg("DST equivalents are written only for tracks and frame ranges"));
        }
        let frame = match state.held.take() {
            Some(f) => f,
            None => {
                let mut disc = d.get_mut()?;
                let frame = state.next_frame(disc.image_mut());
                if let Source::Range(c) = &mut state.source {
                    disc.absorb_seek_checkpoints(c);
                }
                match frame {
                    None => return Ok(SbStatus::End),
                    Some(f) => f?,
                }
            }
        };
        // Stored content and DST equivalents: specification 4, 4.6.1 items 1 to 4.
        let kind = match &frame.content {
            FrameContent::Dsd(_) => SbFrameKind::Dsd,
            FrameContent::Dst(_) => SbFrameKind::Dst,
            FrameContent::Missing => SbFrameKind::Missing,
        };
        let bytes: std::borrow::Cow<'_, [u8]> = match (&frame.content, equivalent) {
            (_, true) => state.dst_equivalent(&frame.content).ok_or_else(|| {
                Failure::arg("DST equivalents are written only for tracks and frame ranges")
            })?,
            (FrameContent::Dsd(b) | FrameContent::Dst(b), false) => std::borrow::Cow::Borrowed(b),
            (FrameContent::Missing, false) => std::borrow::Cow::Borrowed(&[]),
        };
        let size = bytes.len();
        // At most SB_MAX_FRAME_BYTES (a stored frame or its DST equivalent), so it fits.
        let fi = SbRawFrameInfo { number: frame.number, kind: kind as u32, size: size as u32 };
        if !info.is_null() {
            // SAFETY: non-NULL and valid for a write by this function's contract.
            unsafe { info.write(fi) };
        }
        if size > 0 {
            if buf.is_null() || cap < size {
                let msg = format!("frame {} needs {size} bytes", frame.number);
                drop(bytes);
                state.held = Some(frame);
                return Err(Failure(SbStatus::BufferTooSmall, msg));
            }
            // SAFETY: buf is valid for cap >= size bytes of writes; `bytes` is owned by
            // this library, so the regions do not overlap.
            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, size) };
        }
        Ok(SbStatus::Ok)
    })
}

/// The number of missing positions returned so far by `sb_raw_frames_read` (those
/// decoded reads fill with 0x69).
///
/// # Safety
///
/// `raw` must be NULL or a live handle; `missing` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_raw_frames_missing(raw: *const SbRawFrames, missing: *mut u64) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let r = unsafe { raw_handle(raw) }?;
        let state = r.inner.try_borrow().map_err(|_| Failure::arg(REENTRANT))?;
        // A held missing position has been counted by the cursor but not yet returned.
        let held = state.held.as_ref().is_some_and(|f| f.content == FrameContent::Missing);
        // SAFETY: forwarded contract.
        unsafe { write_out(missing, state.missing_frames() - u64::from(held)) }?;
        Ok(SbStatus::Ok)
    })
}

/// The number of malformed audio sectors processed so far by this handle's scan of its
/// area, as `sb_track_audio_malformed_sectors`. The scan stops at the first frame at or
/// after the position asked for, so it may count sectors of that next frame before the
/// frame is returned (no further: a raw frames handle does not read ahead in groups).
///
/// # Safety
///
/// `raw` must be NULL or a live handle; `count` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_raw_frames_malformed_sectors(
    raw: *const SbRawFrames,
    count: *mut u64,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let r = unsafe { raw_handle(raw) }?;
        let state = r.inner.try_borrow().map_err(|_| Failure::arg(REENTRANT))?;
        // SAFETY: forwarded contract.
        unsafe { write_out(count, state.malformed_sectors()) }?;
        Ok(SbStatus::Ok)
    })
}

/// `audio-sector-malformed` warning `index` of this handle's scan of its area so far, as
/// `sb_track_audio_warning`. Returns `SB_STATUS_INVALID_ARGUMENT` past the last one.
///
/// # Safety
///
/// `raw` must be NULL or a live handle; `out` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_raw_frames_warning(
    raw: *const SbRawFrames,
    index: u32,
    out: *mut SbWarning,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let r = unsafe { raw_handle(raw) }?;
        let state = r.inner.try_borrow().map_err(|_| Failure::arg(REENTRANT))?;
        let w =
            state.warnings().get(index as usize).ok_or_else(|| Failure::arg("warning index out of range"))?;
        // SAFETY: forwarded contract.
        unsafe { write_out(out, sb_warning(w)) }?;
        Ok(SbStatus::Ok)
    })
}

/// Frees a raw frames handle. NULL is ignored, and so is a handle whose read is running
/// (the call is then a no-op).
///
/// # Safety
///
/// `raw` must be NULL or a live handle, freed only once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_raw_frames_free(raw: *mut SbRawFrames) {
    let _ = guard(|| {
        // SAFETY: forwarded contract.
        let handle = unsafe { raw_handle(raw) }?;
        if handle.inner.try_borrow_mut().is_err() {
            return Err(Failure::arg(REENTRANT));
        }
        // SAFETY: a live handle created by Box::into_raw and not in use, freed only once.
        drop(unsafe { Box::from_raw(raw) });
        Ok(SbStatus::Ok)
    });
}

/// Decodes one DST frame of `channels` channels (1 to 6) into the first `channels x 4704`
/// bytes of `out`. On `SB_STATUS_INVALID_DST_FRAME`, `*rule` (if not NULL) receives the
/// rule of specification 5, 5.6 and the output bytes are unspecified. A frame is at most
/// `SB_MAX_FRAME_BYTES` long (specification 4, 4.4.3 and 7, 7.6): a longer `len` returns
/// `SB_STATUS_INVALID_ARGUMENT` without reading `frame`.
///
/// # Safety
///
/// `frame` must be NULL (only with `len` 0) or valid for `len` bytes of reads; `out` valid
/// for `out_len` bytes of writes and not overlapping `frame`; `rule` NULL or valid for a
/// write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_dst_decode(
    frame: *const u8,
    len: usize,
    channels: u32,
    out: *mut u8,
    out_len: usize,
    rule: *mut u32,
) -> SbStatus {
    guard(|| {
        let ch = u8::try_from(channels).ok().filter(|c| (1..=6).contains(c));
        let Some(ch) = ch else { return Err(Failure::arg("channels must be 1 to 6")) };
        let need = usize::from(ch) * scarletbook::dst::FL;
        if out.is_null() || out_len < need {
            return Err(Failure(SbStatus::BufferTooSmall, format!("the output needs {need} bytes")));
        }
        if len > SB_MAX_FRAME_BYTES {
            let msg = format!("a DST frame is at most {SB_MAX_FRAME_BYTES} bytes, not {len}");
            return Err(Failure(SbStatus::InvalidArgument, msg));
        }
        let input: &[u8] = if len == 0 {
            &[]
        } else if frame.is_null() {
            return Err(Failure::arg("NULL frame with non-zero length"));
        } else {
            // SAFETY: non-NULL and valid for `len` bytes of reads by this function's contract.
            unsafe { std::slice::from_raw_parts(frame, len) }
        };
        // SAFETY: non-NULL, valid for out_len >= need bytes of writes and not overlapping
        // `frame`, by this function's contract.
        let output = unsafe { std::slice::from_raw_parts_mut(out, need) };
        match scarletbook::dst::decode(input, ch, output) {
            Ok(()) => Ok(SbStatus::Ok),
            Err(e) => {
                if !rule.is_null() {
                    // SAFETY: non-NULL and valid for a write by this function's contract.
                    unsafe { rule.write(u32::from(e.rule())) };
                }
                Err(Failure(SbStatus::InvalidDstFrame, format!("invalid DST frame: {e}")))
            }
        }
    })
}

/// The default checkpoint interval of a seek index, in sectors (`sb_seek_index_new`).
pub const SB_SEEK_DEFAULT_INTERVAL: u32 = 64;

/// The largest checkpoint interval of a seek index, in sectors.
pub const SB_SEEK_MAX_INTERVAL: u32 = 65_536;

/// The default bound on the bytes of frames under assembly a seek index stores (1 MiB).
pub const SB_SEEK_DEFAULT_INLINE_BUDGET: u64 = 1_048_576;

/// An inline budget as the library takes it: one beyond the address space is no bound.
fn budget(inline_budget: u64) -> usize {
    usize::try_from(inline_budget).unwrap_or(usize::MAX)
}

// The constants above are the library's (cbindgen needs literals).
const _: () = {
    assert!(SB_SEEK_DEFAULT_INTERVAL == scarletbook::seek::DEFAULT_INTERVAL);
    assert!(SB_SEEK_MAX_INTERVAL == scarletbook::seek::MAX_INTERVAL);
    assert!(SB_SEEK_DEFAULT_INLINE_BUDGET == scarletbook::seek::DEFAULT_INLINE_BUDGET as u64);
};

/// Makes an empty seek index of the area from `slot` of `disc`, with a checkpoint every
/// `interval` sectors (1 to `SB_SEEK_MAX_INTERVAL`; `SB_SEEK_DEFAULT_INTERVAL` is 64) and
/// at most `inline_budget` bytes of stored frames under assembly
/// (`SB_SEEK_DEFAULT_INLINE_BUDGET`; 0 stores none, which is always correct and only
/// slower on hostile images).
///
/// An index is the reading state of the area: ranges opened with it
/// (`sb_range_audio_open_indexed`, `sb_raw_frames_open_indexed`) start at its last
/// checkpoint before their first frame, with exactly the frames, counts and warnings of a
/// scan from the area's first sector. It fills as handles opened with it read
/// (`sb_track_audio_absorb`, `sb_raw_frames_absorb`), or at once with
/// `sb_seek_index_extend`. It holds about 24 bytes per `interval` audio sectors, plus at
/// most `inline_budget` bytes and 16 KiB (`sb_seek_index_memory`). On failure `*out` is
/// set to NULL: `SB_STATUS_NO_SUCH_AREA`, or `SB_STATUS_INVALID_ARGUMENT` for a bad slot
/// or interval.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `out` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_seek_index_new(
    disc: *const SbDisc,
    slot: u32,
    interval: u32,
    inline_budget: u64,
    out: *mut *mut SbSeekIndex,
) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { clear_out(out) };
    guard(|| {
        // SAFETY: forwarded contract.
        let handle = unsafe { disc_handle(disc) }?;
        if out.is_null() {
            return Err(Failure::arg("NULL output pointer"));
        }
        let d = handle.get()?;
        let area = find_area(&d, slot)?;
        let options = IndexOptions::new().with_interval(interval).with_inline_budget(budget(inline_budget));
        let index = AreaIndex::new(area, d.toc().sectors, options)?;
        let index = Box::new(SbSeekIndex { inner: RefCell::new(index), events: RefCell::default() });
        // SAFETY: non-NULL and valid for a write by this function's contract.
        unsafe { out.write(Box::into_raw(index)) };
        Ok(SbStatus::Ok)
    })
}

/// Reads the index's area on from its frontier, about `max_sectors` sectors (at least to
/// the next checkpoint), recording checkpoints; `*complete` (if not NULL) receives 1 if
/// the index now covers the whole area, else 0. Calling it until complete builds the index in one
/// pass over the area, for example on a second disc handle of the same image on another
/// thread. The disc's read and log callbacks run during this call. Fails with
/// `SB_STATUS_INVALID_ARGUMENT` for an index of another area or image size, one built from
/// another image in this process (see `SbSeekIndex`), or one that does not match the
/// image's sectors; `SB_STATUS_IO` if a read fails (the checkpoints recorded before stay).
///
/// # Safety
///
/// `disc` and `index` must be NULL or live handles; `complete` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_seek_index_extend(
    disc: *const SbDisc,
    index: *mut SbSeekIndex,
    max_sectors: u64,
    complete: *mut u8,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?;
        // SAFETY: forwarded contract.
        let i = unsafe { index_handle(index) }?;
        let mut index = i.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?;
        let mut disc = d.get_mut()?;
        let done = index.extend(disc.image_mut(), max_sectors)?;
        if !complete.is_null() {
            // SAFETY: non-NULL and valid for a write by this function's contract.
            unsafe { complete.write(u8::from(done)) };
        }
        Ok(SbStatus::Ok)
    })
}

/// The LSN up to which the index holds the area's reading state (`*lsn`, required), and
/// whether it covers the whole area (`*complete`, optional: 1 or 0).
///
/// # Safety
///
/// `index` must be NULL or a live handle; `lsn` and `complete` NULL or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_seek_index_frontier(
    index: *const SbSeekIndex,
    lsn: *mut u64,
    complete: *mut u8,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let i = unsafe { index_handle(index) }?;
        let index = i.inner.try_borrow().map_err(|_| Failure::arg(REENTRANT))?;
        // SAFETY: forwarded contract.
        unsafe { write_out(lsn, index.frontier()) }?;
        if !complete.is_null() {
            // SAFETY: non-NULL and valid for a write by this function's contract.
            unsafe { complete.write(u8::from(index.is_complete())) };
        }
        Ok(SbStatus::Ok)
    })
}

/// The bytes the index holds in memory.
///
/// # Safety
///
/// `index` must be NULL or a live handle; `bytes` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_seek_index_memory(index: *const SbSeekIndex, bytes: *mut u64) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let i = unsafe { index_handle(index) }?;
        let index = i.inner.try_borrow().map_err(|_| Failure::arg(REENTRANT))?;
        // SAFETY: forwarded contract.
        unsafe { write_out(bytes, index.memory() as u64) }?;
        Ok(SbStatus::Ok)
    })
}

/// Takes into `into` the checkpoints of `from`, an index of the same area, image size and
/// interval (for example one built on another thread); `into` then reaches the farther of
/// the two frontiers. Fails with `SB_STATUS_INVALID_ARGUMENT`, both indexes unchanged,
/// for indexes of different areas, sizes or intervals, built from two images in this
/// process (see `SbSeekIndex`), or that disagree. Merging an index into itself does
/// nothing.
///
/// # Safety
///
/// `into` and `from` must be NULL or live handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_seek_index_merge(into: *mut SbSeekIndex, from: *const SbSeekIndex) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let a = unsafe { index_handle(into) }?;
        // SAFETY: forwarded contract.
        let b = unsafe { index_handle(from) }?;
        if std::ptr::eq(a, b) {
            return Ok(SbStatus::Ok);
        }
        let mut a = a.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?;
        let b = b.inner.try_borrow().map_err(|_| Failure::arg(REENTRANT))?;
        a.merge(&b)?;
        Ok(SbStatus::Ok)
    })
}

/// Saves the index as bytes, to load it with `sb_seek_index_load` the next time the image
/// is opened. `*len` (required) receives the size; with `buf` NULL or `cap` below it, the
/// call returns `SB_STATUS_BUFFER_TOO_SMALL` and writes nothing else (a size query).
///
/// # Safety
///
/// `index` must be NULL or a live handle; `buf` NULL or valid for `cap` bytes of writes;
/// `len` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_seek_index_save(
    index: *const SbSeekIndex,
    buf: *mut u8,
    cap: usize,
    len: *mut usize,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let i = unsafe { index_handle(index) }?;
        let bytes = i.inner.try_borrow().map_err(|_| Failure::arg(REENTRANT))?.to_bytes();
        // SAFETY: forwarded contract.
        unsafe { write_out(len, bytes.len()) }?;
        if buf.is_null() || cap < bytes.len() {
            return Err(Failure(SbStatus::BufferTooSmall, format!("the index needs {} bytes", bytes.len())));
        }
        // SAFETY: buf is valid for cap >= bytes.len() bytes; `bytes` is owned here.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len()) };
        Ok(SbStatus::Ok)
    })
}

/// Loads a seek index of the area from `slot` of `disc` from `len` bytes saved by
/// `sb_seek_index_save`. The bytes may be anything: every bound and invariant is checked
/// before use, and the memory taken is proportional to `len`. On failure `*out` is set to
/// NULL: `SB_STATUS_INVALID_ARGUMENT` for bytes that are not a valid index of this area
/// and image size, `SB_STATUS_NO_SUCH_AREA`. An index that passes is still checked
/// against the image's sectors as it is used: on a mismatch a handle discards it (a
/// warning in the log) and scans from the area's first sector, with the right result.
///
/// `inline_budget` bounds the bytes of stored frames under assembly the loaded index holds
/// (`SB_SEEK_DEFAULT_INLINE_BUDGET` is 1 MiB, as for `sb_seek_index_new`): its budget is the
/// smaller of the saved one and this, and stored frames beyond it are dropped (their
/// checkpoints replayed instead, with the same results).
///
/// The checks make loading safe, not exact: bytes forged to pass them can make reads
/// return other frames than the image holds. If exact output matters, rebuild an index
/// read from untrusted storage (`sb_seek_index_new` and `sb_seek_index_extend`) instead.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `buf` NULL or valid for `len` bytes of reads;
/// `out` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_seek_index_load(
    disc: *const SbDisc,
    slot: u32,
    buf: *const u8,
    len: usize,
    inline_budget: u64,
    out: *mut *mut SbSeekIndex,
) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { clear_out(out) };
    guard(|| {
        // SAFETY: forwarded contract.
        let handle = unsafe { disc_handle(disc) }?;
        if out.is_null() {
            return Err(Failure::arg("NULL output pointer"));
        }
        if buf.is_null() {
            return Err(Failure::arg("NULL buffer"));
        }
        // SAFETY: non-NULL and valid for `len` bytes of reads by this function's contract.
        let bytes = unsafe { std::slice::from_raw_parts(buf, len) };
        let d = handle.get()?;
        let area = find_area(&d, slot)?;
        let index = AreaIndex::from_bytes(area, d.toc().sectors, bytes, budget(inline_budget))?;
        let index = Box::new(SbSeekIndex { inner: RefCell::new(index), events: RefCell::default() });
        // SAFETY: non-NULL and valid for a write by this function's contract.
        unsafe { out.write(Box::into_raw(index)) };
        Ok(SbStatus::Ok)
    })
}

/// Frees a seek index. NULL is ignored. Handles opened with it hold no reference to it.
/// A call from one of a disc's callbacks while the index is in use by the call that
/// invoked the callback does nothing.
///
/// # Safety
///
/// `index` must be NULL or a live handle, freed only once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_seek_index_free(index: *mut SbSeekIndex) {
    let _ = guard(|| {
        // SAFETY: forwarded contract.
        let handle = unsafe { index_handle(index) }?;
        if handle.inner.try_borrow_mut().is_err() {
            return Err(Failure::arg(REENTRANT));
        }
        // SAFETY: a live handle created by Box::into_raw and not in use, freed only once.
        drop(unsafe { Box::from_raw(index) });
        Ok(SbStatus::Ok)
    });
}

/// Keeps a seek index of the area from `slot` in `disc`, empty to begin with (see
/// `sb_seek_index_new` for `interval` and `inline_budget`), replacing one kept before:
/// from then on every track audio and raw frames handle of that area opened through the
/// disc or any of its clones (`sb_disc_clone`, `sb_disc_clone_callbacks`) starts from it,
/// and reads through them (`sb_track_audio_read`, `sb_raw_frames_read`) and continuations
/// (`_open_next`) build it. The disc and its clones keep one index, used and grown from any thread; a
/// handle's frames, statuses, counts and warnings are those without an index. Fails with
/// `SB_STATUS_NO_SUCH_AREA` or `SB_STATUS_INVALID_ARGUMENT`.
///
/// # Safety
///
/// `disc` must be NULL or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_seek_index_enable(
    disc: *const SbDisc,
    slot: u32,
    interval: u32,
    inline_budget: u64,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?;
        let options = IndexOptions::new().with_interval(interval).with_inline_budget(budget(inline_budget));
        d.get_mut()?.enable_seek_index(slot_from(slot)?, options)?;
        Ok(SbStatus::Ok)
    })
}

/// Keeps a copy of `index` (for example one loaded with `sb_seek_index_load`) as the seek
/// index of its area in `disc` and its clones, as `sb_disc_seek_index_enable` does an
/// empty one. Fails with `SB_STATUS_INVALID_ARGUMENT` for an index of another area or
/// image size, or `SB_STATUS_NO_SUCH_AREA`.
///
/// # Safety
///
/// `disc` and `index` must be NULL or live handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_seek_index_set(
    disc: *const SbDisc,
    slot: u32,
    index: *const SbSeekIndex,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?;
        // SAFETY: forwarded contract.
        let i = unsafe { index_handle(index) }?;
        let copy = i.inner.try_borrow().map_err(|_| Failure::arg(REENTRANT))?.clone();
        d.get_mut()?.set_seek_index(slot_from(slot)?, copy)?;
        Ok(SbStatus::Ok)
    })
}

/// A copy of the seek index `disc` (and its clones) keeps for the area from `slot`, as a
/// new handle in `*out`, for example to save it (`sb_seek_index_save`). Returns
/// `SB_STATUS_ABSENT`, with `*out` NULL, when none is kept.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `out` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_seek_index_get(
    disc: *const SbDisc,
    slot: u32,
    out: *mut *mut SbSeekIndex,
) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { clear_out(out) };
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?;
        if out.is_null() {
            return Err(Failure::arg("NULL output pointer"));
        }
        let Some(index) = d.get()?.seek_index(slot_from(slot)?) else { return Ok(SbStatus::Absent) };
        let index = Box::new(SbSeekIndex { inner: RefCell::new(index), events: RefCell::default() });
        // SAFETY: non-NULL and valid for a write by this function's contract.
        unsafe { out.write(Box::into_raw(index)) };
        Ok(SbStatus::Ok)
    })
}

/// Stops keeping a seek index of the area from `slot` in `disc` and its clones.
///
/// # Safety
///
/// `disc` must be NULL or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_seek_index_disable(disc: *const SbDisc, slot: u32) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?;
        d.get_mut()?.take_seek_index(slot_from(slot)?);
        Ok(SbStatus::Ok)
    })
}

/// Reads the area from `slot` on from the frontier of the seek index `disc` keeps, about
/// `max_sectors` sectors, building it (see `sb_seek_index_extend`); `*complete` (if not
/// NULL) receives 1 if it now covers the whole area, else 0. Call it on a clone in a thread of its
/// own to build the index while other clones read. Fails with
/// `SB_STATUS_INVALID_ARGUMENT` when no index is kept or it does not match the image,
/// `SB_STATUS_IO` if a read fails.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `complete` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_seek_index_extend(
    disc: *const SbDisc,
    slot: u32,
    max_sectors: u64,
    complete: *mut u8,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?;
        let done = d.get_mut()?.extend_seek_index(slot_from(slot)?, max_sectors)?;
        if !complete.is_null() {
            // SAFETY: non-NULL and valid for a write by this function's contract.
            unsafe { complete.write(u8::from(done)) };
        }
        Ok(SbStatus::Ok)
    })
}

/// As `sb_range_audio_open`, starting from `index` (a seek index of the area, or NULL for
/// none): at its last checkpoint before frame `first`, with exactly the frames, counts and
/// warnings of `sb_range_audio_open`. The handle copies what it needs and holds no
/// reference to `index`; it records the checkpoints it crosses past the index's frontier,
/// which `sb_track_audio_absorb` hands to an index. Fails as `sb_range_audio_open`, or
/// with `SB_STATUS_INVALID_ARGUMENT` for an index of another area or image size.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `index` NULL or a live handle; `out` NULL or
/// valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_range_audio_open_indexed(
    disc: *const SbDisc,
    slot: u32,
    first: u32,
    end: u32,
    index: *const SbSeekIndex,
    out: *mut *mut SbTrackAudio,
) -> SbStatus {
    // SAFETY: forwarded contract; a NULL `prev` is never consumed, and the index is only
    // borrowed (a mutable borrow of a shared handle, checked by its RefCell).
    unsafe { open_next(disc, std::ptr::null_mut(), slot, Target::Range(first, end), index.cast_mut(), out) }
}

/// As `sb_range_audio_open_next`, with `index` (NULL for none): `prev`'s recorded
/// checkpoints are first taken into `index`, then `prev` is continued where that gives the
/// same result, or the range starts from the index. Ownership of `prev` is as for
/// `sb_track_audio_open_next`; every check, the index's included, comes before `prev` is
/// taken.
///
/// # Safety
///
/// As `sb_range_audio_open_next`; `index` NULL or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_range_audio_open_next_indexed(
    disc: *const SbDisc,
    prev: *mut SbTrackAudio,
    slot: u32,
    first: u32,
    end: u32,
    index: *mut SbSeekIndex,
    out: *mut *mut SbTrackAudio,
) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { open_next(disc, prev, slot, Target::Range(first, end), index, out) }
}

/// As `sb_raw_frames_open`, starting from `index` (NULL for none), as
/// `sb_range_audio_open_indexed` does for decoded audio.
///
/// # Safety
///
/// As `sb_range_audio_open_indexed`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_raw_frames_open_indexed(
    disc: *const SbDisc,
    slot: u32,
    first: u32,
    end: u32,
    index: *const SbSeekIndex,
    out: *mut *mut SbRawFrames,
) -> SbStatus {
    // SAFETY: forwarded contract (see `sb_range_audio_open_indexed`).
    unsafe { raw_open_next(disc, std::ptr::null_mut(), slot, (first, end), index.cast_mut(), out) }
}

/// As `sb_raw_frames_open_next`, with `index` (NULL for none), as
/// `sb_range_audio_open_next_indexed` does for decoded audio.
///
/// # Safety
///
/// As `sb_raw_frames_open_next`; `index` NULL or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_raw_frames_open_next_indexed(
    disc: *const SbDisc,
    prev: *mut SbRawFrames,
    slot: u32,
    first: u32,
    end: u32,
    index: *mut SbSeekIndex,
    out: *mut *mut SbRawFrames,
) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { raw_open_next(disc, prev, slot, (first, end), index, out) }
}

/// Takes into `index` the checkpoints a track audio handle recorded past the index's
/// frontier (one opened with an index; others recorded none, and this does nothing). Call
/// it whenever convenient, for example before freeing the handle; the handle then drops
/// what it recorded. Fails with `SB_STATUS_INVALID_ARGUMENT`, `index` unchanged, if the
/// handle read another image than the one the index was built from in this process (see
/// `SbSeekIndex`; the handle then stops recording), or its checkpoints are of another
/// area, image size or interval, or do not join the index.
///
/// # Safety
///
/// `audio` and `index` must be NULL or live handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_track_audio_absorb(
    audio: *const SbTrackAudio,
    index: *mut SbSeekIndex,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let a = unsafe { audio_handle(audio) }?;
        // SAFETY: forwarded contract.
        let i = unsafe { index_handle(index) }?;
        let mut cursor = a.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?;
        let mut index = i.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?;
        index.absorb(&mut *cursor)?;
        Ok(SbStatus::Ok)
    })
}

/// As `sb_track_audio_absorb`, for a raw frames handle (a stored frame list's records
/// none).
///
/// # Safety
///
/// `raw` and `index` must be NULL or live handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_raw_frames_absorb(raw: *const SbRawFrames, index: *mut SbSeekIndex) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let r = unsafe { raw_handle(raw) }?;
        // SAFETY: forwarded contract.
        let i = unsafe { index_handle(index) }?;
        let mut raw = r.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?;
        let mut index = i.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?;
        match &mut raw.source {
            Source::Range(c) => index.absorb(c)?,
            Source::List(..) => {}
        }
        Ok(SbStatus::Ok)
    })
}

/// `sb_disc_seek_policy_set`: exact reads only (the default).
pub const SB_SEEK_POLICY_EXACT: u32 = 0;
/// `sb_disc_seek_policy_set`: provisional reading allowed (specification 4, 4.2 item 8).
pub const SB_SEEK_POLICY_SPECULATIVE: u32 = 1;
/// The default `min_gap_sectors` of `sb_disc_seek_policy_set` (4 MiB of sectors).
pub const SB_SEEK_DEFAULT_MIN_GAP: u64 = 2048;

/// `SbSpeculation.state`: exact positions, not a provisional reading.
pub const SB_SPECULATION_EXACT: u32 = 0;
/// `SbSpeculation.state`: provisional positions, not yet checked.
pub const SB_SPECULATION_PENDING: u32 = 1;
/// `SbSpeculation.state`: provisional positions confirmed: they are the specification's.
pub const SB_SPECULATION_CONFIRMED: u32 = 2;
/// `SbSpeculation.state`: provisional positions possibly different from the specification's
/// below `exact_from`.
pub const SB_SPECULATION_REFUTED: u32 = 3;
/// `SbSpeculation.state`: provisional positions that will not be checked.
pub const SB_SPECULATION_UNVERIFIED: u32 = 4;

/// Whether a handle's positions are provisional (specification 4, 4.2 item 8, sacd-spec
/// `82eac6c`), or the verdict an event reports (`sb_disc_seek_event_next`).
///
/// Layout: 40 bytes, aligned to 8, without padding (2 fields of 8 bytes, then 6 of 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SbSpeculation {
    /// The provisional reading's number (0 for exact positions): events and handles name it
    /// alike.
    pub claim: u64,
    /// `L`, the sector the provisional reading began at (0 before the handle's first read,
    /// which is when it chooses, and for exact positions). The malformed count and warnings
    /// of a provisional handle begin there (4.4.5 item 5).
    pub lsn: u64,
    /// The area's slot.
    pub slot: u32,
    /// `SB_SPECULATION_*`.
    pub state: u32,
    /// `u`, the first position read provisionally.
    pub first: u32,
    /// For `SB_SPECULATION_REFUTED`: the last position returned, `UINT32_MAX` if none.
    pub last_returned: u32,
    /// For `SB_SPECULATION_REFUTED`: positions from this frame on are exact (positions
    /// `first` to `exact_from - 1` are possibly different, whether or not missing).
    pub exact_from: u32,
    /// 0.
    pub reserved: u32,
}

impl SbSpeculation {
    fn of(slot: Slot, claim: Option<u64>, s: Speculation) -> Self {
        let slot = slot_index(slot);
        let claim = claim.unwrap_or(0);
        let none = u32::MAX;
        let (state, lsn, first, last_returned, exact_from) = match s {
            Speculation::Pending { lsn, first } => (SB_SPECULATION_PENDING, lsn, first, none, 0),
            Speculation::Confirmed { lsn, first } => (SB_SPECULATION_CONFIRMED, lsn, first, none, 0),
            Speculation::Refuted { lsn, first, last_returned, exact_from } => {
                (SB_SPECULATION_REFUTED, lsn, first, last_returned.unwrap_or(none), exact_from)
            }
            Speculation::Unverified { lsn, first } => (SB_SPECULATION_UNVERIFIED, lsn, first, none, 0),
            Speculation::Exact => (SB_SPECULATION_EXACT, 0, 0, none, 0),
            // A state this interface does not know yet is never reported as exact.
            _ => (SB_SPECULATION_UNVERIFIED, 0, 0, none, 0),
        };
        SbSpeculation { claim, lsn, slot, state, first, last_returned, exact_from, reserved: 0 }
    }
}

/// Sets how `disc` starts ranges of the area from `slot` that its seek index (if any)
/// cannot reach cheaply: `SB_SEEK_POLICY_EXACT` (the default) or
/// `SB_SEEK_POLICY_SPECULATIVE`, which lets a handle begin a range far into the area after a
/// short search of the frame timecodes instead of reading the area up to it
/// (**provisional reading**, specification 4, 4.2 item 8, sacd-spec `82eac6c`).
///
/// Opting in is accepting provisional positions: they equal a straight read whenever no
/// frame numbered at or after the range was accepted before the provisional start and the
/// frame under assembly there is none or numbered below the range (always on a well-formed
/// disc), but no reader can know that without reading the sectors before the range, and
/// on hostile images they can differ. Query a handle with `sb_track_audio_speculation`
/// after its first read; with a seek index kept (`sb_disc_seek_index_enable`), provisional
/// readings are checked as the index is built (`sb_disc_seek_verify`) and their verdicts
/// returned by `sb_disc_seek_event_next`.
///
/// A provisional start is used when the exact start lies more than `min_gap_sectors`
/// (`SB_SEEK_DEFAULT_MIN_GAP`) before the range's estimated position; 0 allows one whenever
/// the exact start lies before it at all. `min_gap_sectors` is ignored with
/// `SB_SEEK_POLICY_EXACT`. The policy is the
/// handle's own: clones (`sb_disc_clone`) start with a copy. Fails with
/// `SB_STATUS_NO_SUCH_AREA`, or `SB_STATUS_INVALID_ARGUMENT` for another `policy`.
///
/// # Safety
///
/// `disc` must be NULL or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_seek_policy_set(
    disc: *const SbDisc,
    slot: u32,
    policy: u32,
    min_gap_sectors: u64,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?;
        let policy = match policy {
            SB_SEEK_POLICY_EXACT => SeekPolicy::Exact,
            SB_SEEK_POLICY_SPECULATIVE => {
                SeekPolicy::Speculative(SpeculativeOptions::new().with_min_gap_sectors(min_gap_sectors))
            }
            _ => {
                return Err(Failure::arg(
                    "policy must be SB_SEEK_POLICY_EXACT or SB_SEEK_POLICY_SPECULATIVE",
                ));
            }
        };
        d.get_mut()?.set_seek_policy(slot_from(slot)?, policy)?;
        Ok(SbStatus::Ok)
    })
}

/// Writes to `*out` whether `audio`'s positions are provisional (see
/// `sb_disc_seek_policy_set`): `SB_SPECULATION_EXACT`, or the state of its provisional
/// reading. A handle chooses its start on its first read: query it after that.
///
/// # Safety
///
/// `audio` must be NULL or a live handle; `out` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_track_audio_speculation(
    audio: *const SbTrackAudio,
    out: *mut SbSpeculation,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let a = unsafe { audio_handle(audio) }?;
        let c = a.inner.try_borrow().map_err(|_| Failure::arg(REENTRANT))?;
        let slot = c.slot();
        let value = SbSpeculation::of(slot, c.speculation_id(), c.speculation());
        // SAFETY: forwarded contract.
        unsafe { write_out(out, value) }?;
        Ok(SbStatus::Ok)
    })
}

/// As `sb_track_audio_speculation`, for a raw frames handle (a stored frame list's are
/// exact).
///
/// # Safety
///
/// `raw` must be NULL or a live handle; `out` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_raw_frames_speculation(
    raw: *const SbRawFrames,
    out: *mut SbSpeculation,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let r = unsafe { raw_handle(raw) }?;
        let raw = r.inner.try_borrow().map_err(|_| Failure::arg(REENTRANT))?;
        let value = match &raw.source {
            Source::Range(c) => SbSpeculation::of(c.slot(), c.speculation_id(), c.speculation()),
            Source::List(..) => SbSpeculation::default(),
        };
        // SAFETY: forwarded contract.
        unsafe { write_out(out, value) }?;
        Ok(SbStatus::Ok)
    })
}

/// Builds the seek index `disc` keeps for the area from `slot` by about `max_sectors` more
/// sectors and checks the provisional readings it then reaches: as
/// `sb_disc_seek_index_extend`, whose verdicts `sb_disc_seek_event_next` returns. `*complete`
/// (if not NULL) is 1 once the index covers the whole area: every provisional reading of
/// the area has its verdict then. Call it on a clone in a thread of its own while other
/// clones read. Fails as `sb_disc_seek_index_extend`.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `complete` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_seek_verify(
    disc: *const SbDisc,
    slot: u32,
    max_sectors: u64,
    complete: *mut u8,
) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { sb_disc_seek_index_extend(disc, slot, max_sectors, complete) }
}

/// The next verdict on a provisional reading of the area from `slot` of `disc` or any of its
/// clones, in `*out`: `SB_SPECULATION_CONFIRMED`, `SB_SPECULATION_REFUTED` (drop or re-read
/// what was served from positions `first` to `exact_from - 1`; an exact read of them is now
/// cheap) or `SB_SPECULATION_UNVERIFIED`. Returns `SB_STATUS_END` when there is none. The
/// disc and its clones share one queue, which keeps the last 1 024 verdicts; each verdict
/// is returned once, by whichever handle asks first. A call moves the verdicts queued so
/// far to the asking handle, which returns them one by one: verdicts it has not returned
/// when it is freed are lost (a handle's own reports, `sb_track_audio_speculation`, keep
/// them).
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `out` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_seek_event_next(
    disc: *const SbDisc,
    slot: u32,
    out: *mut SbSpeculation,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?;
        if out.is_null() {
            return Err(Failure::arg("NULL output pointer"));
        }
        let slot = slot_from(slot)?;
        let at = slot_index(slot) as usize;
        let mut events = d.events.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?;
        if events[at].is_empty() {
            events[at].extend(d.get()?.take_seek_events(slot));
        }
        let Some(e) = events[at].pop_front() else { return Ok(SbStatus::End) };
        // SAFETY: non-NULL and valid for a write by this function's contract.
        unsafe { out.write(SbSpeculation::of(e.slot, Some(e.claim), e.speculation)) };
        Ok(SbStatus::Ok)
    })
}

/// Checks the provisional readings a caller-kept seek index has taken
/// (`sb_track_audio_absorb`) and that its checkpoints now reach, reading through `disc` (a
/// disc of the index's image); `*settled` (if not NULL) receives the number checked. Their
/// verdicts are returned by `sb_seek_index_event_next`; `sb_seek_index_extend` checks them
/// too. Fails with `SB_STATUS_INVALID_ARGUMENT` for a disc of another image (see
/// `SbSeekIndex`), `SB_STATUS_IO` if a read fails.
///
/// # Safety
///
/// `disc` and `index` must be NULL or live handles; `settled` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_seek_index_verify(
    disc: *const SbDisc,
    index: *mut SbSeekIndex,
    settled: *mut u32,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?;
        // SAFETY: forwarded contract.
        let i = unsafe { index_handle(index) }?;
        let mut index = i.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?;
        let mut disc = d.get_mut()?;
        let n = index.verify(disc.image_mut())?;
        if !settled.is_null() {
            // SAFETY: non-NULL and valid for a write by this function's contract.
            unsafe { settled.write(u32::try_from(n).unwrap_or(u32::MAX)) };
        }
        Ok(SbStatus::Ok)
    })
}

/// The next verdict on a provisional reading a caller-kept seek index has checked, in
/// `*out` (see `sb_disc_seek_event_next`); `SB_STATUS_END` when there is none.
///
/// # Safety
///
/// `index` must be NULL or a live handle; `out` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_seek_index_event_next(
    index: *const SbSeekIndex,
    out: *mut SbSpeculation,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let i = unsafe { index_handle(index) }?;
        if out.is_null() {
            return Err(Failure::arg("NULL output pointer"));
        }
        let mut events = i.events.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?;
        if events.is_empty() {
            events.extend(i.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?.take_events());
        }
        let Some(e) = events.pop_front() else { return Ok(SbStatus::End) };
        // SAFETY: non-NULL and valid for a write by this function's contract.
        unsafe { out.write(SbSpeculation::of(e.slot, Some(e.claim), e.speculation)) };
        Ok(SbStatus::Ok)
    })
}

/// Hands out the cancellation request of `disc`'s reads, in `*out`: a handle of its own,
/// for another thread to cancel them with `sb_cancel_request`. A disc and its clones
/// (`sb_disc_clone*`) have the same request, so cancelling stops the reads of all of
/// them. Call it on the disc's thread (it uses the disc), before handing the token to the
/// thread that cancels. On failure `*out` is set to NULL.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `out` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_cancel_token(disc: *const SbDisc, out: *mut *mut SbCancel) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { clear_out(out) };
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?;
        if out.is_null() {
            return Err(Failure::arg("NULL output pointer"));
        }
        let token = d.get()?.cancel_token();
        // SAFETY: non-NULL and valid for a write by this function's contract.
        unsafe { out.write(Box::into_raw(Box::new(SbCancel { token }))) };
        Ok(SbStatus::Ok)
    })
}

/// Borrows a cancellation token.
///
/// # Safety
///
/// `token` must be NULL or a live handle from `sb_disc_cancel_token`.
unsafe fn cancel_handle<'a>(token: *const SbCancel) -> Result<&'a SbCancel, Failure> {
    // SAFETY: NULL or a live handle by this function's contract; only shared references
    // to it are ever made, and it is Sync.
    unsafe { token.as_ref() }.ok_or_else(|| Failure::arg("NULL cancellation token"))
}

/// Requests, from any thread, that the reads of the token's disc and its clones stop.
/// Until `sb_cancel_withdraw`, every call that reads audio sectors through them
/// (`sb_track_audio_read`, `sb_raw_frames_read`, the seek index extension and verification
/// calls) returns `SB_STATUS_CANCELLED`: a call that starts then at once, a call in
/// progress after at most 32 more sectors read (or, during the reads sized to the range
/// after a provisional start, after the read in progress, of at most 512 sectors, 1 MiB),
/// plus the decoding of one group of frames
/// already started for the frame it is returning. The handles keep their state, frames
/// read ahead included: once the request is withdrawn they continue exactly where they
/// were (a provisional search, `SB_SEEK_POLICY_SPECULATIVE`, cancelled before it chose its
/// start starts again). They can also be freed. Opening, cloning and the calls that read
/// no audio sectors are not cancelled.
///
/// The request is sticky: it stays, for the disc and every clone, until
/// `sb_cancel_withdraw`, so a caller that keeps the disc withdraws it before reading
/// again. A disc opened afresh (`sb_disc_open_*`), even of the same file, has a token of its
/// own and is not affected.
///
/// # Safety
///
/// `token` must be NULL or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_cancel_request(token: *const SbCancel) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        unsafe { cancel_handle(token) }?.token.cancel();
        Ok(SbStatus::Ok)
    })
}

/// Withdraws the request of `sb_cancel_request`, from any thread: reads go on again.
///
/// # Safety
///
/// `token` must be NULL or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_cancel_withdraw(token: *const SbCancel) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        unsafe { cancel_handle(token) }?.token.withdraw();
        Ok(SbStatus::Ok)
    })
}

/// Whether cancellation is requested: `*requested` is 1 or 0.
///
/// # Safety
///
/// `token` must be NULL or a live handle; `requested` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_cancel_requested(token: *const SbCancel, requested: *mut u8) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let t = unsafe { cancel_handle(token) }?;
        // SAFETY: forwarded contract.
        unsafe { write_out(requested, u8::from(t.token.is_cancelled())) }?;
        Ok(SbStatus::Ok)
    })
}

/// Frees a cancellation token. NULL is ignored. It may outlive its disc; freeing it
/// neither requests nor withdraws anything.
///
/// # Safety
///
/// `token` must be NULL or a live handle, freed only once, and not in use by another call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_cancel_free(token: *mut SbCancel) {
    let _ = guard(|| {
        if !token.is_null() {
            // SAFETY: a live handle created by Box::into_raw, not in use, and freed only
            // once (this function's contract).
            drop(unsafe { Box::from_raw(token) });
        }
        Ok(SbStatus::Ok)
    });
}

/// Writes a new progress handle of `progress` to `out`.
///
/// # Safety
///
/// `out` must be valid for a write.
unsafe fn write_progress(out: *mut *mut SbProgress, progress: Progress) {
    // SAFETY: valid for a write by this function's contract.
    unsafe { out.write(Box::into_raw(Box::new(SbProgress { progress }))) };
}

/// Hands out how far a track or range audio handle has read, in `*out`: a handle of its
/// own, which `sb_progress_get` reads from any thread while `audio` reads. Call it on the
/// handle's thread, when the handle is not in use (it uses the handle). Every call for
/// one handle follows the same counts; a handle from `sb_track_audio_open_next` or
/// `sb_range_audio_open_next` counts afresh, and needs one of its own. On failure `*out`
/// is set to NULL.
///
/// # Safety
///
/// `audio` must be NULL or a live handle; `out` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_track_audio_progress(
    audio: *const SbTrackAudio,
    out: *mut *mut SbProgress,
) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { clear_out(out) };
    guard(|| {
        // SAFETY: forwarded contract.
        let a = unsafe { audio_handle(audio) }?;
        if out.is_null() {
            return Err(Failure::arg("NULL output pointer"));
        }
        let progress = a.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?.progress();
        // SAFETY: non-NULL and valid for a write by this function's contract.
        unsafe { write_progress(out, progress) };
        Ok(SbStatus::Ok)
    })
}

/// As `sb_track_audio_progress`, for a raw frames handle (of a range, or of an area's
/// stored frame list); a handle from `sb_raw_frames_open_next` counts afresh.
///
/// # Safety
///
/// `raw` must be NULL or a live handle; `out` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_raw_frames_progress(
    raw: *const SbRawFrames,
    out: *mut *mut SbProgress,
) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { clear_out(out) };
    guard(|| {
        // SAFETY: forwarded contract.
        let r = unsafe { raw_handle(raw) }?;
        if out.is_null() {
            return Err(Failure::arg("NULL output pointer"));
        }
        let progress = r.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))?.progress();
        // SAFETY: non-NULL and valid for a write by this function's contract.
        unsafe { write_progress(out, progress) };
        Ok(SbStatus::Ok)
    })
}

/// How far the handle has read, from any thread: `*sectors_read`, the sectors it has read
/// through its disc so far (exact), and `*sectors_expected`, the sectors it expects to
/// have read once done (an estimate placing the range's end in proportion to the area's
/// play time, revised as it reads; at least `*sectors_read`, and equal to it once the
/// handle has read its range to the end). They count every read the handle makes, at most
/// 32 sectors at a time (except the reads sized to the range after a provisional start, of
/// at most 512 each): the scan before a range's first frame, a provisional search, a
/// seek index's replay and the range itself. Both outputs are required.
///
/// # Safety
///
/// `progress` must be NULL or a live handle; `sectors_read` and `sectors_expected` NULL or
/// valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_progress_get(
    progress: *const SbProgress,
    sectors_read: *mut u64,
    sectors_expected: *mut u64,
) -> SbStatus {
    guard(|| {
        // SAFETY: NULL or a live handle by this function's contract; only shared references
        // to it are ever made, and it is Sync.
        let p = unsafe { progress.as_ref() }.ok_or_else(|| Failure::arg("NULL progress handle"))?;
        if sectors_read.is_null() || sectors_expected.is_null() {
            return Err(Failure::arg("NULL output pointer"));
        }
        let (read, expected) = p.progress.get();
        // SAFETY: forwarded contract (checked non-NULL above).
        unsafe { write_out(sectors_read, read) }?;
        // SAFETY: forwarded contract (checked non-NULL above).
        unsafe { write_out(sectors_expected, expected) }?;
        Ok(SbStatus::Ok)
    })
}

/// Frees a progress handle. NULL is ignored. It may outlive the handle it follows.
///
/// # Safety
///
/// `progress` must be NULL or a live handle, freed only once, and not in use by another
/// call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_progress_free(progress: *mut SbProgress) {
    let _ = guard(|| {
        if !progress.is_null() {
            // SAFETY: a live handle created by Box::into_raw, not in use, and freed only
            // once (this function's contract).
            drop(unsafe { Box::from_raw(progress) });
        }
        Ok(SbStatus::Ok)
    });
}

/// The first 8 bytes of a `sb_disc_toc` layout (`SbTocHeader.magic`).
const TOC_MAGIC: &[u8; 8] = b"SBTOC\0\0\0";
/// The major version of the `sb_disc_toc` layout this library writes. A reader refuses a
/// layout of any other major version.
pub const SB_TOC_MAJOR: u16 = 1;
/// The minor version of the `sb_disc_toc` layout this library writes. Minor versions only
/// append fields to the end of records (the record sizes in `SbTocHeader` grow); every
/// offset of an earlier minor version stays.
pub const SB_TOC_MINOR: u16 = 0;
/// `SbTocStr.offset` of an absent string.
pub const SB_TOC_ABSENT: u32 = 0xFFFF_FFFF;

/// A string in a `sb_disc_toc` layout: `length` bytes at byte `offset` of the layout,
/// followed by a NUL that `length` does not count; `offset` is `SB_TOC_ABSENT` (and
/// `length` 0) for an absent string, where the per-field call returns
/// `SB_STATUS_ABSENT`. Text is UTF-8, as from `sb_disc_string`; stored catalogue numbers
/// and the manufacturer sector are bytes as stored.
///
/// Layout: 8 bytes, aligned to 4: `offset` at 0, `length` at 4.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SbTocStr {
    /// Byte offset from the start of the layout, or `SB_TOC_ABSENT`.
    pub offset: u32,
    /// Length in bytes, without the trailing NUL.
    pub length: u32,
}

/// The header of a `sb_disc_toc` layout, at offset 0.
///
/// Layout: 56 bytes, aligned to 4: `magic` at 0, `major` at 8, `minor` at 10, `size` at
/// 12, `disc_offset` at 16, the four record sizes at 20, 22, 24 and 26, `areas_offset` at
/// 28, `area_count` at 32, `warnings_offset` at 36, `warning_count` at 40,
/// `strings_offset` at 44, `strings_size` at 48, `reserved` at 52.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SbTocHeader {
    /// The bytes "SBTOC" and three NULs.
    pub magic: [u8; 8],
    /// `SB_TOC_MAJOR` of the library that wrote the layout.
    pub major: u16,
    /// `SB_TOC_MINOR` of the library that wrote the layout.
    pub minor: u16,
    /// The layout's size in bytes (what `*len` receives).
    pub size: u32,
    /// Offset of the `SbTocDisc` record.
    pub disc_offset: u32,
    /// Size of the disc record (256 in format 1.0).
    pub disc_record_size: u16,
    /// Size of each area record (152 in format 1.0).
    pub area_record_size: u16,
    /// Size of each track record (168 in format 1.0).
    pub track_record_size: u16,
    /// Size of each warning record (24 in format 1.0).
    pub warning_record_size: u16,
    /// Offset of the first area record; the others follow, `area_record_size` apart, in
    /// `sb_disc_area` index order.
    pub areas_offset: u32,
    /// Number of area records (`SbDiscInfo.area_count`).
    pub area_count: u32,
    /// Offset of the first warning record; the others follow, `warning_record_size` apart,
    /// in `sb_disc_warning` index order.
    pub warnings_offset: u32,
    /// Number of warning records (`SbDiscInfo.warning_count`).
    pub warning_count: u32,
    /// Offset of the string area, where every `SbTocStr` points.
    pub strings_offset: u32,
    /// Size of the string area.
    pub strings_size: u32,
    /// Written as 0.
    pub reserved: u32,
}

/// The disc record of a `sb_disc_toc` layout.
///
/// Layout (format 1.0): 256 bytes, aligned to 8: `info` at 0, `album_genres` at 32,
/// `disc_genres` at 40, `text_channel_count` at 48, `text_charset` at 49, `disc_flags` at
/// 50, `reserved` at 51, `text_channels` at 52, `reserved2` at 84, `strings` at 88,
/// `catalog_numbers_stored` at 232, `manufacturer` at 248.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SbTocDisc {
    /// As `sb_disc_info`.
    pub info: SbDiscInfo,
    /// As `sb_disc_genre` with `SB_GENRE_LIST_ALBUM`, indexes 0 to 3.
    pub album_genres: [SbGenre; 4],
    /// As `sb_disc_genre` with `SB_GENRE_LIST_DISC`, indexes 0 to 3.
    pub disc_genres: [SbGenre; 4],
    /// As `sb_disc_text_channel_count`, as stored (may exceed 8).
    pub text_channel_count: u8,
    /// The master TOC's text character set code byte, as stored.
    pub text_charset: u8,
    /// The disc flags byte, as stored (`SbDiscInfo.hybrid` is its bit 7).
    pub disc_flags: u8,
    /// Written as 0.
    pub reserved: u8,
    /// As `sb_disc_text_channel`, for the first `min(text_channel_count, 8)`; zero after.
    pub text_channels: [SbTextChannel; 8],
    /// Written as 0.
    pub reserved2: u32,
    /// As `sb_disc_string`, fields 0 to 17 (`SbDiscString`).
    pub strings: [SbTocStr; 18],
    /// As `sb_disc_catalog_number_stored`: the album's, then the disc's.
    pub catalog_numbers_stored: [SbTocStr; 2],
    /// As `sb_disc_manufacturer`: absent when `info.manufacturer_sector` is 0.
    pub manufacturer: SbTocStr,
}

/// An area record of a `sb_disc_toc` layout.
///
/// Layout (format 1.0): 152 bytes, aligned to 4: `info` at 0, `details` at 52,
/// `loudspeaker_layout` at 60, `channel_speakers` at 64, `playtime_stored` at 70,
/// `reserved` at 73, `reserved2` at 74, `tracks_offset` at 76, `strings` at 80,
/// `text_channels` at 112.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SbTocArea {
    /// As `sb_disc_area`.
    pub info: SbAreaInfo,
    /// As `sb_area_details`.
    pub details: SbAreaDetails,
    /// As `sb_area_loudspeaker_layout`.
    pub loudspeaker_layout: u32,
    /// As `sb_area_channel_speaker`, for the first `min(info.channel_count, 6)`; 0 after.
    pub channel_speakers: [u8; 6],
    /// As `sb_area_playtime_stored`.
    pub playtime_stored: SbTimecode,
    /// Written as 0.
    pub reserved: u8,
    /// Written as 0.
    pub reserved2: u16,
    /// Offset of the area's first track record; the others follow, `track_record_size`
    /// apart, `info.track_count` in all, in track number order from 1.
    pub tracks_offset: u32,
    /// As `sb_area_string`, fields 0 to 3 (`SbAreaString`).
    pub strings: [SbTocStr; 4],
    /// As `sb_area_text_channel`, for the first `min(details.text_channel_count, 10)`;
    /// zero after.
    pub text_channels: [SbTextChannel; 10],
}

/// A track record of a `sb_disc_toc` layout.
///
/// Layout (format 1.0): 168 bytes, aligned to 4: `info` at 0, `start_stored` at 28,
/// `duration_stored` at 31, `reserved` at 34, `strings` at 36, `reserved2` at 164.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SbTocTrack {
    /// As `sb_track_info`.
    pub info: SbTrackInfo,
    /// As `sb_track_timecodes_stored`: the start.
    pub start_stored: SbTimecode,
    /// As `sb_track_timecodes_stored`: the duration.
    pub duration_stored: SbTimecode,
    /// Written as 0.
    pub reserved: [u8; 2],
    /// As `sb_track_string`, fields 0 to 14 (`SbTrackString`); entry 15 is reserved and
    /// always absent.
    pub strings: [SbTocStr; 16],
    /// Written as 0; keeps records a multiple of 8 bytes.
    pub reserved2: u32,
}

/// A warning record of a `sb_disc_toc` layout.
///
/// Layout (format 1.0): 24 bytes, aligned to 8: `lsn` at 0, `slot` at 8, `track` at 12,
/// `code` at 16.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SbTocWarning {
    /// As `SbWarning.lsn`.
    pub lsn: i64,
    /// As `SbWarning.slot`.
    pub slot: i32,
    /// As `SbWarning.track`.
    pub track: u32,
    /// The code's name, as `SbWarning.code`.
    pub code: SbTocStr,
}

// Format 1.0 of the layout; a change here is a change of the documented format.
const _: () = {
    assert!(size_of::<SbTocStr>() == 8);
    assert!(size_of::<SbTocHeader>() == 56);
    assert!(size_of::<SbTocDisc>() == 256);
    assert!(size_of::<SbTocArea>() == 152);
    assert!(size_of::<SbTocTrack>() == 168);
    assert!(size_of::<SbTocWarning>() == 24);
};

/// Writes little-endian values at offsets of a byte buffer.
struct LayoutWriter<'a> {
    buf: &'a mut [u8],
    /// Where the next string goes.
    strings_at: usize,
}

impl LayoutWriter<'_> {
    fn bytes(&mut self, at: usize, bytes: &[u8]) -> Result<(), Failure> {
        let end =
            at.checked_add(bytes.len()).ok_or_else(|| Failure(SbStatus::Panic, "layout offset".into()))?;
        self.buf
            .get_mut(at..end)
            .ok_or_else(|| Failure(SbStatus::Panic, "layout field out of range".into()))?
            .copy_from_slice(bytes);
        Ok(())
    }
    fn u8(&mut self, at: usize, v: u8) -> Result<(), Failure> {
        self.bytes(at, &[v])
    }
    fn u16(&mut self, at: usize, v: u16) -> Result<(), Failure> {
        self.bytes(at, &v.to_le_bytes())
    }
    fn u32(&mut self, at: usize, v: u32) -> Result<(), Failure> {
        self.bytes(at, &v.to_le_bytes())
    }
    fn u64(&mut self, at: usize, v: u64) -> Result<(), Failure> {
        self.bytes(at, &v.to_le_bytes())
    }

    /// Appends `s` (and a NUL) to the string area and writes its `SbTocStr` at `at`.
    fn str(&mut self, at: usize, s: Option<&[u8]>) -> Result<(), Failure> {
        let r = match s {
            None => SbTocStr { offset: SB_TOC_ABSENT, length: 0 },
            Some(s) => {
                let offset = self.strings_at;
                self.bytes(offset, s)?;
                self.u8(offset + s.len(), 0)?;
                self.strings_at = offset + s.len() + 1;
                SbTocStr { offset: layout_u32(offset)?, length: layout_u32(s.len())? }
            }
        };
        self.u32(at + offset_of!(SbTocStr, offset), r.offset)?;
        self.u32(at + offset_of!(SbTocStr, length), r.length)
    }

    fn timecode(&mut self, at: usize, t: SbTimecode) -> Result<(), Failure> {
        self.bytes(at, &[t.minutes, t.seconds, t.frames])
    }

    fn text_channels(
        &mut self,
        at: usize,
        locales: &[scarletbook::toc::Locale],
        max: usize,
    ) -> Result<(), Failure> {
        for (i, l) in locales.iter().take(max).enumerate() {
            let c = at + i * size_of::<SbTextChannel>();
            self.bytes(c + offset_of!(SbTextChannel, language), &l.language_code)?;
            self.u8(c + offset_of!(SbTextChannel, charset), l.charset)?;
        }
        Ok(())
    }
}

fn layout_u32(n: usize) -> Result<u32, Failure> {
    u32::try_from(n).map_err(|_| Failure(SbStatus::Panic, "layout larger than 4 GiB".into()))
}

/// Every string of a disc's layout, in the order they are written to the string area;
/// `f` gets each (`None`: absent).
fn layout_strings<'a>(d: &'a Disc<Reader>, mut f: impl FnMut(Option<&'a [u8]>)) -> Result<(), Failure> {
    let m = &d.toc().master;
    for field in 0..18 {
        f(disc_string(m, field)?.map(str::as_bytes));
    }
    f(Some(&m.album_catalog_number_stored));
    f(Some(&m.disc_catalog_number_stored));
    f(m.manufacturer.as_deref());
    for a in &d.toc().areas {
        for item in AreaTextItem::ALL {
            f(a.text.get(*item).map(str::as_bytes));
        }
    }
    for a in &d.toc().areas {
        for t in &a.tracks {
            for field in 0..15 {
                f(track_string(t, field)?.map(str::as_bytes));
            }
            f(None);
        }
    }
    for w in d.warnings() {
        f(Some(warning_code(w.code).to_bytes()));
    }
    Ok(())
}

/// The record offsets of a disc's layout.
struct LayoutPlan {
    areas: usize,
    tracks: usize,
    warnings: usize,
    strings: usize,
    strings_size: usize,
    size: usize,
}

impl LayoutPlan {
    /// Every count is bounded (2 areas, 255 tracks each, strings of the TOC sectors), so
    /// the sums fit a `usize`; the size must fit the `u32` offsets (it is far below).
    fn of(d: &Disc<Reader>) -> Result<Self, Failure> {
        let areas = size_of::<SbTocHeader>() + size_of::<SbTocDisc>();
        let tracks = areas + d.toc().areas.len() * size_of::<SbTocArea>();
        let track_count: usize = d.toc().areas.iter().map(|a| a.tracks.len()).sum();
        let warnings = tracks + track_count * size_of::<SbTocTrack>();
        let strings = warnings + d.warnings().len() * size_of::<SbTocWarning>();
        let mut strings_size = 0usize;
        layout_strings(d, |s| strings_size += s.map_or(0, |s| s.len() + 1))?;
        let size = strings + strings_size;
        layout_u32(size)?;
        Ok(LayoutPlan { areas, tracks, warnings, strings, strings_size, size })
    }
}

/// Writes a disc's layout into `buf`, which is `plan.size` bytes.
fn write_layout(d: &Disc<Reader>, plan: &LayoutPlan, buf: &mut [u8]) -> Result<(), Failure> {
    buf.fill(0);
    let mut w = LayoutWriter { buf, strings_at: plan.strings };
    let toc = d.toc();
    let m = &toc.master;

    // Header.
    w.bytes(offset_of!(SbTocHeader, magic), TOC_MAGIC)?;
    w.u16(offset_of!(SbTocHeader, major), SB_TOC_MAJOR)?;
    w.u16(offset_of!(SbTocHeader, minor), SB_TOC_MINOR)?;
    w.u32(offset_of!(SbTocHeader, size), layout_u32(plan.size)?)?;
    w.u32(offset_of!(SbTocHeader, disc_offset), layout_u32(size_of::<SbTocHeader>())?)?;
    w.u16(offset_of!(SbTocHeader, disc_record_size), size_of::<SbTocDisc>() as u16)?;
    w.u16(offset_of!(SbTocHeader, area_record_size), size_of::<SbTocArea>() as u16)?;
    w.u16(offset_of!(SbTocHeader, track_record_size), size_of::<SbTocTrack>() as u16)?;
    w.u16(offset_of!(SbTocHeader, warning_record_size), size_of::<SbTocWarning>() as u16)?;
    w.u32(offset_of!(SbTocHeader, areas_offset), layout_u32(plan.areas)?)?;
    w.u32(offset_of!(SbTocHeader, area_count), layout_u32(toc.areas.len())?)?;
    w.u32(offset_of!(SbTocHeader, warnings_offset), layout_u32(plan.warnings)?)?;
    w.u32(offset_of!(SbTocHeader, warning_count), layout_u32(d.warnings().len())?)?;
    w.u32(offset_of!(SbTocHeader, strings_offset), layout_u32(plan.strings)?)?;
    w.u32(offset_of!(SbTocHeader, strings_size), layout_u32(plan.strings_size)?)?;

    // Disc record.
    let at = size_of::<SbTocHeader>();
    let info = disc_info(d);
    let i = at + offset_of!(SbTocDisc, info);
    w.u64(i + offset_of!(SbDiscInfo, sectors), info.sectors)?;
    w.u32(i + offset_of!(SbDiscInfo, sector_size), info.sector_size)?;
    w.u8(i + offset_of!(SbDiscInfo, version_major), info.version_major)?;
    w.u8(i + offset_of!(SbDiscInfo, version_minor), info.version_minor)?;
    w.u16(i + offset_of!(SbDiscInfo, album_set_size), info.album_set_size)?;
    w.u16(i + offset_of!(SbDiscInfo, album_sequence_number), info.album_sequence_number)?;
    w.u8(i + offset_of!(SbDiscInfo, hybrid), info.hybrid)?;
    w.u8(i + offset_of!(SbDiscInfo, manufacturer_sector), info.manufacturer_sector)?;
    w.u16(i + offset_of!(SbDiscInfo, year), info.year)?;
    w.u8(i + offset_of!(SbDiscInfo, month), info.month)?;
    w.u8(i + offset_of!(SbDiscInfo, day), info.day)?;
    w.u32(i + offset_of!(SbDiscInfo, area_count), info.area_count)?;
    w.u32(i + offset_of!(SbDiscInfo, warning_count), info.warning_count)?;
    for (field, genres) in [
        (offset_of!(SbTocDisc, album_genres), &m.album_genres),
        (offset_of!(SbTocDisc, disc_genres), &m.disc_genres),
    ] {
        for (k, g) in genres.iter().enumerate() {
            w.bytes(at + field + k * size_of::<SbGenre>(), &[g.category, g.index])?;
        }
    }
    w.u8(at + offset_of!(SbTocDisc, text_channel_count), m.text_channel_count)?;
    w.u8(at + offset_of!(SbTocDisc, text_charset), m.text_charset)?;
    w.u8(at + offset_of!(SbTocDisc, disc_flags), m.disc_flags)?;
    w.text_channels(at + offset_of!(SbTocDisc, text_channels), &m.locales, 8)?;
    // The strings, in the order of `layout_strings`.
    for field in 0..18 {
        let s = disc_string(m, field)?.map(str::as_bytes);
        w.str(at + offset_of!(SbTocDisc, strings) + field as usize * size_of::<SbTocStr>(), s)?;
    }
    let c = at + offset_of!(SbTocDisc, catalog_numbers_stored);
    w.str(c, Some(&m.album_catalog_number_stored))?;
    w.str(c + size_of::<SbTocStr>(), Some(&m.disc_catalog_number_stored))?;
    w.str(at + offset_of!(SbTocDisc, manufacturer), m.manufacturer.as_deref())?;

    // Area records.
    let mut track_at = plan.tracks;
    for (k, a) in toc.areas.iter().enumerate() {
        let at = plan.areas + k * size_of::<SbTocArea>();
        let info = area_info(a);
        let i = at + offset_of!(SbTocArea, info);
        for (field, v) in [
            (offset_of!(SbAreaInfo, slot), info.slot),
            (offset_of!(SbAreaInfo, kind_channels), info.kind_channels),
            (offset_of!(SbAreaInfo, toc_lsn), info.toc_lsn),
            (offset_of!(SbAreaInfo, toc_sectors), info.toc_sectors),
            (offset_of!(SbAreaInfo, channel_count), info.channel_count),
            (offset_of!(SbAreaInfo, area_config), info.area_config),
            (offset_of!(SbAreaInfo, sample_frequency_code), info.sample_frequency_code),
            (offset_of!(SbAreaInfo, frame_format), info.frame_format),
            (offset_of!(SbAreaInfo, max_byte_rate), info.max_byte_rate),
            (offset_of!(SbAreaInfo, playtime_frames), info.playtime_frames),
            (offset_of!(SbAreaInfo, audio_start_lsn), info.audio_start_lsn),
            (offset_of!(SbAreaInfo, audio_end_lsn), info.audio_end_lsn),
            (offset_of!(SbAreaInfo, track_count), info.track_count),
        ] {
            w.u32(i + field, v)?;
        }
        let x = area_details(a);
        w.bytes(
            at + offset_of!(SbTocArea, details),
            &[
                x.version_major,
                x.version_minor,
                x.max_available_channels,
                x.mute_flags,
                x.track_attribute,
                x.track_offset,
                x.text_channel_count,
                x.text_charset,
            ],
        )?;
        w.u32(at + offset_of!(SbTocArea, loudspeaker_layout), layout_value(a))?;
        for c in 0..usize::from(a.channel_count).min(6) {
            w.u8(at + offset_of!(SbTocArea, channel_speakers) + c, speaker_value(a, c) as u8)?;
        }
        w.timecode(at + offset_of!(SbTocArea, playtime_stored), SbTimecode::from(a.playtime))?;
        w.u32(at + offset_of!(SbTocArea, tracks_offset), layout_u32(track_at)?)?;
        w.text_channels(at + offset_of!(SbTocArea, text_channels), &a.locales, 10)?;
        for (f, item) in AreaTextItem::ALL.iter().enumerate() {
            let s = a.text.get(*item).map(str::as_bytes);
            w.str(at + offset_of!(SbTocArea, strings) + f * size_of::<SbTocStr>(), s)?;
        }
        track_at += a.tracks.len() * size_of::<SbTocTrack>();
    }

    // Track records, area by area.
    let mut at = plan.tracks;
    for a in &toc.areas {
        for t in &a.tracks {
            let info = track_info(t);
            let i = at + offset_of!(SbTocTrack, info);
            for (field, v) in [
                (offset_of!(SbTrackInfo, number), info.number),
                (offset_of!(SbTrackInfo, start_lsn), info.start_lsn),
                (offset_of!(SbTrackInfo, length_sectors), info.length_sectors),
                (offset_of!(SbTrackInfo, start_frame), info.start_frame),
                (offset_of!(SbTrackInfo, duration_frames), info.duration_frames),
            ] {
                w.u32(i + field, v)?;
            }
            w.bytes(
                i + offset_of!(SbTrackInfo, start_flags),
                &[
                    info.start_flags,
                    info.duration_flags,
                    info.has_genre,
                    info.genre_category,
                    info.genre_index,
                ],
            )?;
            let (start, duration) = d.track_timecodes_stored(a.slot, u32::from(t.number))?;
            w.timecode(at + offset_of!(SbTocTrack, start_stored), SbTimecode::from(start))?;
            w.timecode(at + offset_of!(SbTocTrack, duration_stored), SbTimecode::from(duration))?;
            for field in 0..16u32 {
                let s = if field < 15 { track_string(t, field)?.map(str::as_bytes) } else { None };
                w.str(at + offset_of!(SbTocTrack, strings) + field as usize * size_of::<SbTocStr>(), s)?;
            }
            at += size_of::<SbTocTrack>();
        }
    }

    // Warning records.
    for (k, warning) in d.warnings().iter().enumerate() {
        let at = plan.warnings + k * size_of::<SbTocWarning>();
        let sw = sb_warning(warning);
        w.u64(at + offset_of!(SbTocWarning, lsn), sw.lsn as u64)?;
        w.u32(at + offset_of!(SbTocWarning, slot), sw.slot as u32)?;
        w.u32(at + offset_of!(SbTocWarning, track), sw.track)?;
        w.str(at + offset_of!(SbTocWarning, code), Some(warning_code(warning.code).to_bytes()))?;
    }

    if w.strings_at != plan.size {
        return Err(Failure(SbStatus::Panic, "layout strings differ from the plan".into()));
    }
    Ok(())
}

/// The whole TOC of a disc in one buffer, in a fixed, versioned binary layout: what the
/// disc, area, track, string, genre, text channel, timecode and warning queries return,
/// for every area and track, without a call per field. The layout is described by
/// `SbTocHeader` (at offset 0), `SbTocDisc`, `SbTocArea`, `SbTocTrack`, `SbTocWarning` and
/// `SbTocStr`: every integer is little-endian whatever the host, every field is at a fixed
/// offset that is a multiple of its size, and a buffer aligned to 8 bytes can be read
/// through these structs on a little-endian host. The README describes it, with a ctypes
/// example.
///
/// `flags` must be 0. `*len` (required) receives the layout's size, the same on every call
/// for a disc and its clones. If `buf` is NULL or `cap < *len`, nothing is written and the
/// call returns `SB_STATUS_BUFFER_TOO_SMALL`; otherwise the layout is written to
/// `buf[0..*len]` and the call returns `SB_STATUS_OK`. Recommended: pass a 64 KiB buffer
/// (enough for typical discs) and call again with `*len` bytes on
/// `SB_STATUS_BUFFER_TOO_SMALL`.
///
/// No I/O and no log output: the data is the TOC parsed when the disc was opened.
///
/// # Safety
///
/// `disc` must be NULL or a live handle; `buf` NULL or valid for `cap` bytes of writes;
/// `len` NULL or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_toc(
    disc: *const SbDisc,
    flags: u32,
    buf: *mut u8,
    cap: usize,
    len: *mut usize,
) -> SbStatus {
    guard(|| {
        // SAFETY: forwarded contract.
        let d = unsafe { disc_handle(disc) }?.get()?;
        if len.is_null() {
            return Err(Failure::arg("NULL output pointer"));
        }
        if flags != 0 {
            return Err(Failure::arg("flags must be 0"));
        }
        let plan = LayoutPlan::of(&d)?;
        // SAFETY: forwarded contract (checked non-NULL above).
        unsafe { write_out(len, plan.size) }?;
        if buf.is_null() || cap < plan.size {
            return Ok(SbStatus::BufferTooSmall);
        }
        // SAFETY: `buf` is valid for `cap >= plan.size` bytes of writes by the contract and
        // is the caller's memory, which no reference of this library aliases.
        let out = unsafe { std::slice::from_raw_parts_mut(buf, plan.size) };
        write_layout(&d, &plan, out)?;
        Ok(SbStatus::Ok)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_gap_is_the_librarys() {
        assert_eq!(SB_SEEK_DEFAULT_MIN_GAP, scarletbook::SpeculativeOptions::default().min_gap_sectors);
    }

    #[test]
    fn guard_turns_a_panic_into_a_status() {
        let status = guard(|| -> Result<SbStatus, Failure> {
            #[allow(clippy::panic)]
            {
                panic!("deliberate")
            }
        });
        assert_eq!(status, SbStatus::Panic);
        // SAFETY: sb_last_error_message returns a valid NUL-terminated string.
        let msg = unsafe { CStr::from_ptr(sb_last_error_message()) };
        assert!(msg.to_bytes().starts_with(b"internal error"));
        // The next call clears the message.
        assert_eq!(guard(|| Ok(SbStatus::Ok)), SbStatus::Ok);
        // SAFETY: as above.
        let msg = unsafe { CStr::from_ptr(sb_last_error_message()) };
        assert!(msg.to_bytes().is_empty());
    }
}
