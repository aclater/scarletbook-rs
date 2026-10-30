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
use std::io::{self, BufReader, Cursor, Read, Seek, SeekFrom};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};

use scarletbook::toc::{AreaTextItem, DiscTextItem, TrackTextItem};
use scarletbook::{Area, Disc, Error, FrameStatus, Slot, TrackAudioCursor, WarningCode};

/// Result of every call.
#[repr(C)]
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
}

/// Area slots, as passed in `slot` arguments.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub enum SbSlot {
    /// The 2-channel slot.
    TwoChannel = 0,
    /// The multichannel slot.
    Multichannel = 1,
}

/// How a decoded frame was produced (`SbFrameInfo.status`).
#[repr(C)]
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

/// Disc-level strings for `sb_disc_string`: the 16 disc text items of specification 2,
/// 2.5.4, then the two catalogue numbers.
#[repr(C)]
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
#[repr(C)]
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
#[repr(C)]
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
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SbDiscInfo {
    /// Stored sector size: 2048, 2054 or 2064.
    pub sector_size: u32,
    /// Number of logical sectors in the image.
    pub sectors: u64,
    /// Master TOC version.
    pub version_major: u8,
    /// Master TOC version.
    pub version_minor: u8,
    /// Album set size.
    pub album_set_size: u16,
    /// Album sequence number.
    pub album_sequence_number: u16,
    /// Hybrid disc flag.
    pub hybrid: bool,
    /// Whether the manufacturer sector is present.
    pub manufacturer_sector: bool,
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
    /// Whether the area has genres (`SACD_IGL`).
    pub has_genre: bool,
    /// Genre table byte.
    pub genre_category: u8,
    /// Genre index byte.
    pub genre_index: u8,
}

/// A warning.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SbWarning {
    /// The diagnostic code of specification 7 (a static string).
    pub code: *const c_char,
    /// `SbSlot`, or -1 if the warning concerns no area.
    pub slot: i32,
    /// Track number, or 0 if the warning concerns no track.
    pub track: u32,
    /// The LSN the warning concerns (`audio-sector-malformed`), or -1.
    pub lsn: i64,
}

/// Information about one decoded frame.
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
    pub size: usize,
}

type Reader = Box<dyn ReadSeek + Send>;

/// An opened disc image.
///
/// Its state sits in a `RefCell`: every call takes a shared reference to the handle and
/// borrows the state, so a call made from inside one of this disc's callbacks finds the
/// state borrowed and fails cleanly instead of aliasing it.
pub struct SbDisc {
    inner: RefCell<Disc<Reader>>,
    id: u64,
}

impl std::fmt::Debug for SbDisc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SbDisc").field("id", &self.id).finish_non_exhaustive()
    }
}

/// The decoding state of one track. Each `sb_track_audio_read` call reads through the
/// disc it was opened from; the disc must be live for those calls (it may be freed
/// before the track handle, which can then only be freed).
#[derive(Debug)]
pub struct SbTrackAudio {
    inner: RefCell<TrackAudioCursor>,
    disc_id: u64,
}

// Handles may be moved between threads (the conventions allow it); this must keep
// compiling.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<SbDisc>();
    assert_send::<SbTrackAudio>();
};

trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

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
            Error::InvalidArgument(_) => SbStatus::InvalidArgument,
            Error::Io(_) => SbStatus::Io,
            _ => SbStatus::Panic,
        };
        Failure(status, e.to_string())
    }
}

fn set_error(msg: &str) {
    let c = CString::new(msg.replace('\0', " ")).unwrap_or_default();
    LAST_ERROR.with(|e| *e.borrow_mut() = c);
}

/// Runs `f`, catching panics and recording the error message.
fn guard(f: impl FnOnce() -> Result<SbStatus, Failure>) -> SbStatus {
    set_error("");
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

impl SbDisc {
    fn get(&self) -> Result<Ref<'_, Disc<Reader>>, Failure> {
        self.inner.try_borrow().map_err(|_| Failure::arg(REENTRANT))
    }

    fn get_mut(&self) -> Result<RefMut<'_, Disc<Reader>>, Failure> {
        self.inner.try_borrow_mut().map_err(|_| Failure::arg(REENTRANT))
    }
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
    // SAFETY: forwarded contract; `len` is required whatever the string.
    unsafe { write_out(len, s.map_or(0, str::len)) }?;
    let Some(s) = s else { return Ok(SbStatus::Absent) };
    if buf.is_null() || cap <= s.len() {
        return Ok(SbStatus::BufferTooSmall);
    }
    let bytes = s.as_bytes();
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
unsafe fn open_reader(reader: Reader, out: *mut *mut SbDisc) -> Result<SbStatus, Failure> {
    if out.is_null() {
        return Err(Failure::arg("NULL output pointer"));
    }
    let disc = Disc::open(reader)?;
    let handle =
        Box::new(SbDisc { inner: RefCell::new(disc), id: NEXT_DISC_ID.fetch_add(1, Ordering::Relaxed) });
    // SAFETY: `out` is non-NULL and valid for a write by this function's contract.
    unsafe { out.write(Box::into_raw(handle)) };
    Ok(SbStatus::Ok)
}

/// The version of this C ABI. It is incremented on every incompatible change to a
/// function signature or to the layout of a struct or enum in this header. 1 is the
/// first released ABI.
pub const SB_ABI_VERSION: u32 = 1;

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

/// The message of the calling thread's last failed call (empty if the last call
/// succeeded). Valid until the thread's next call into this library.
#[unsafe(no_mangle)]
pub extern "C" fn sb_last_error_message() -> *const c_char {
    LAST_ERROR.with(|e| e.borrow().as_ptr())
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
    // SAFETY: forwarded contract.
    unsafe { clear_out(out) };
    guard(|| {
        if path.is_null() {
            return Err(Failure::arg("NULL path"));
        }
        // SAFETY: non-NULL and NUL-terminated by this function's contract.
        let path = unsafe { CStr::from_ptr(path) };
        let path = path.to_str().map_err(|_| Failure::arg("path is not UTF-8"))?;
        let file = File::open(path).map_err(|e| Failure(SbStatus::Io, format!("{path}: {e}")))?;
        // SAFETY: forwarded contract.
        unsafe { open_reader(Box::new(BufReader::new(file)), out) }
    })
}

/// Opens a disc image held in memory. The bytes are copied; the caller may free `data`
/// when the call returns. On failure `*out` is set to NULL.
///
/// # Safety
///
/// `data` must be NULL (only with `len` 0) or valid for `len` bytes of reads; `out` NULL
/// or valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_disc_open_memory(data: *const u8, len: usize, out: *mut *mut SbDisc) -> SbStatus {
    // SAFETY: forwarded contract.
    unsafe { clear_out(out) };
    guard(|| {
        let bytes = if len == 0 {
            Vec::new()
        } else if data.is_null() {
            return Err(Failure::arg("NULL data with non-zero length"));
        } else {
            // SAFETY: non-NULL and valid for `len` bytes of reads by this function's contract.
            unsafe { std::slice::from_raw_parts(data, len) }.to_vec()
        };
        // SAFETY: forwarded contract.
        unsafe { open_reader(Box::new(Cursor::new(bytes)), out) }
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

/// Opens a disc image read through callbacks. On failure `*out` is set to NULL.
///
/// The callbacks run during this call and during `sb_track_audio_read`, on the calling
/// thread. A callback that calls back into this library with this disc or one of its
/// track handles gets `SB_STATUS_INVALID_ARGUMENT` (and `sb_disc_free` /
/// `sb_track_audio_free` do nothing); other handles may be used.
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
    // SAFETY: forwarded contract.
    unsafe { clear_out(out) };
    guard(|| {
        let (Some(read), Some(seek)) = (read, seek) else {
            return Err(Failure::arg("NULL callback"));
        };
        // SAFETY: forwarded contract.
        unsafe { open_reader(Box::new(Callbacks { read, seek, user }), out) }
    })
}

/// Frees a disc. NULL is ignored, and so is a disc whose callback is running (the call is
/// then a no-op). Track audio handles opened from it can afterwards only be freed.
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
        let toc = d.toc();
        let m = &toc.master;
        let info = SbDiscInfo {
            sector_size: toc.sector_format.stored_size() as u32,
            sectors: toc.sectors,
            version_major: m.version.major,
            version_minor: m.version.minor,
            album_set_size: m.album_set_size,
            album_sequence_number: m.album_sequence_number,
            hybrid: m.hybrid(),
            manufacturer_sector: m.manufacturer.is_some(),
            year: m.date.year,
            month: m.date.month,
            day: m.date.day,
            area_count: toc.areas.len() as u32,
            warning_count: u32::try_from(d.warnings().len()).unwrap_or(u32::MAX),
        };
        // SAFETY: forwarded contract.
        unsafe { write_out(out, info) }?;
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
        let m = &d.toc().master;
        let s = match field {
            16 => Some(m.album_catalog_number.as_str()),
            17 => Some(m.disc_catalog_number.as_str()),
            f => {
                let item =
                    DiscTextItem::ALL.get(f as usize).ok_or_else(|| Failure::arg("unknown disc string"))?;
                m.text.get(*item)
            }
        };
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
        let info = SbAreaInfo {
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
        };
        // SAFETY: forwarded contract.
        unsafe { write_out(out, info) }?;
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
        let info = SbTrackInfo {
            number: u32::from(t.number),
            start_lsn: t.start_lsn,
            length_sectors: t.length_sectors,
            start_frame: t.start_frame,
            duration_frames: t.duration_frames,
            start_flags: t.start_flags,
            duration_flags: t.duration_flags,
            has_genre: t.genre.is_some(),
            genre_category: t.genre.map_or(0, |g| g.category),
            genre_index: t.genre.map_or(0, |g| g.index),
        };
        // SAFETY: forwarded contract.
        unsafe { write_out(out, info) }?;
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
        let s = if field == 14 {
            t.isrc.as_deref()
        } else {
            let item =
                TrackTextItem::ALL.get(field as usize).ok_or_else(|| Failure::arg("unknown track string"))?;
            t.text.get(*item)
        };
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
        code: warning_code(w.code).as_ptr(),
        slot: w.slot.map_or(-1, |s| slot_index(s) as i32),
        track: w.track.map_or(0, u32::from),
        lsn: w.lsn.map_or(-1, i64::from),
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
        let audio = Box::new(SbTrackAudio { inner: RefCell::new(cursor), disc_id: handle.id });
        // SAFETY: non-NULL and valid for a write by this function's contract.
        unsafe { out.write(Box::into_raw(audio)) };
        Ok(SbStatus::Ok)
    })
}

/// Opens the decoded audio of track `number` of the area from `slot`, continuing `prev`
/// (an earlier track's handle from the same disc) without rescanning the area where the
/// result is the same. Reading an area's tracks in increasing order this way reads each
/// audio sector once. With `prev` NULL this is `sb_track_audio_open`.
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
    unsafe { clear_out(out) };
    guard(|| {
        // SAFETY: forwarded contract.
        let handle = unsafe { disc_handle(disc) }?;
        if out.is_null() {
            return Err(Failure::arg("NULL output pointer"));
        }
        let slot = slot_from(slot)?;
        if prev.is_null() {
            let cursor = handle.get()?.track_audio_cursor(slot, number)?;
            let audio = Box::new(SbTrackAudio { inner: RefCell::new(cursor), disc_id: handle.id });
            // SAFETY: non-NULL and valid for a write by this function's contract.
            unsafe { out.write(Box::into_raw(audio)) };
            return Ok(SbStatus::Ok);
        }
        // Every check that can fail comes before `prev` is taken, so a failure leaves it
        // untouched.
        // SAFETY: forwarded contract.
        let p = unsafe { audio_handle(prev) }?;
        if p.disc_id != handle.id {
            return Err(Failure::arg("the previous track audio handle belongs to another disc"));
        }
        if p.inner.try_borrow_mut().is_err() {
            return Err(Failure::arg(REENTRANT));
        }
        let d = handle.get()?;
        d.check_track(slot, number)?;
        // SAFETY: a live handle created by Box::into_raw and not in use; the caller does
        // not use it again after this successful call.
        let prev_box = unsafe { Box::from_raw(prev) };
        // The track exists and passed check_track, which is the only way continuing can
        // fail.
        let cursor = d.continue_track_audio(prev_box.inner.into_inner(), slot, number)?;
        let audio = Box::new(SbTrackAudio { inner: RefCell::new(cursor), disc_id: handle.id });
        // SAFETY: non-NULL and valid for a write by this function's contract.
        unsafe { out.write(Box::into_raw(audio)) };
        Ok(SbStatus::Ok)
    })
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
/// was opened from. The disc's read callbacks, if any, run during this call.
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
        let mut disc = d.get_mut()?;
        let Some(frame) = cursor.next_frame(disc.image_mut()) else {
            return Ok(SbStatus::End);
        };
        let frame = frame?;
        let (status, rule) = match frame.status {
            FrameStatus::Dsd => (SbFrameStatus::Dsd, 0),
            FrameStatus::Decoded => (SbFrameStatus::Decoded, 0),
            FrameStatus::Missing => (SbFrameStatus::Missing, 0),
            FrameStatus::Invalid(e) => (SbFrameStatus::Invalid, u32::from(e.rule())),
        };
        let n = frame.data.len().min(size);
        // SAFETY: buf is valid for cap >= size >= n bytes of writes; frame.data is owned
        // here, so the regions do not overlap.
        unsafe { std::ptr::copy_nonoverlapping(frame.data.as_ptr(), buf, n) };
        let fi = SbFrameInfo { number: frame.number, status: status as u32, dst_rule: rule, size: n };
        if !info.is_null() {
            // SAFETY: non-NULL and valid for a write by this function's contract.
            unsafe { info.write(fi) };
        }
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
        // SAFETY: forwarded contract (checked non-NULL above).
        unsafe { write_out(missing, cursor.missing_frames()) }?;
        // SAFETY: forwarded contract (checked non-NULL above).
        unsafe { write_out(invalid, cursor.invalid_dst_frames()) }?;
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

/// Decodes one DST frame of `channels` channels (1 to 6) into the first `channels x 4704`
/// bytes of `out`. On `SB_STATUS_INVALID_DST_FRAME`, `*rule` (if not NULL) receives the
/// rule of specification 5, 5.6 and the output bytes are unspecified.
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

#[cfg(test)]
mod tests {
    use super::*;

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
