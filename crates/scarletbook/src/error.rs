// SPDX-License-Identifier: MIT OR Apache-2.0

//! Errors and diagnostics (specification 0, 0.8 and specification 7, 7.7).

use std::fmt;
use std::io;

/// A master TOC area slot (specification 3, 3.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Slot {
    /// The 2-channel slot (master TOC offsets 64, 68 and 84).
    TwoChannel,
    /// The multichannel slot (master TOC offsets 72, 76 and 86).
    Multichannel,
}

impl Slot {
    /// Both slots, in processing order.
    pub const ALL: [Slot; 2] = [Slot::TwoChannel, Slot::Multichannel];

    /// The slot name used by the conformance corpus: `2ch` or `multichannel`.
    pub fn name(self) -> &'static str {
        match self {
            Slot::TwoChannel => "2ch",
            Slot::Multichannel => "multichannel",
        }
    }
}

impl fmt::Display for Slot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// An error that stops an operation.
///
/// The first four variants are the image rejections of specification 7, 7.7.1. The
/// track variants are the per-track outcomes of 7.7.4.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// No `SACDMTOC` signature at LSN 510 in any sector format (`not-scarletbook`).
    #[error("not a Scarletbook image: no SACDMTOC at LSN 510")]
    NotScarletbook,
    /// The image ends before LSN 519 is complete (`master-toc-truncated`).
    #[error("master TOC truncated: the image ends before LSN 519")]
    MasterTocTruncated,
    /// The master TOC version major number is above 1 (`master-toc-version-unsupported`).
    #[error("master TOC version {major}.{minor} is not supported")]
    MasterTocVersionUnsupported {
        /// Stored major version.
        major: u8,
        /// Stored minor version.
        minor: u8,
    },
    /// No area slot yields an area (`no-audio-areas`).
    #[error("no readable audio area")]
    NoAudioAreas,
    /// The area's channel count is not 1 to 6 (`channel-count-unsupported`).
    #[error("track refused: the area has {channels} channels, only 1 to 6 are supported")]
    ChannelCountUnsupported {
        /// The area's stored channel count.
        channels: u8,
    },
    /// The track's duration is 0 (`track-empty`).
    #[error("track skipped: its duration is 0 frames")]
    TrackEmpty,
    /// The track (or frame range) ends beyond 7 frames per existing audio sector
    /// (`track-exceeds-capacity`).
    #[error("refused: it ends at frame {end_frame}, beyond the area capacity of {capacity} frames")]
    TrackExceedsCapacity {
        /// The track's (or range's) end frame number.
        end_frame: u32,
        /// The area's capacity in frames.
        capacity: u64,
    },
    /// The requested area is not in the TOC.
    #[error("no area in slot {0}")]
    NoSuchArea(Slot),
    /// The requested track does not exist in the area.
    #[error("no track {track} in area {slot}")]
    NoSuchTrack {
        /// The area's slot.
        slot: Slot,
        /// The requested track number.
        track: u32,
    },
    /// A requested frame range `[first, end)` is empty or extends past the area's play
    /// time: it must satisfy `first < end <= playtime`.
    #[error("frame range [{first}, {end}) is not within the area's timeline [0, {playtime})")]
    FrameRangeInvalid {
        /// The range's first frame number.
        first: u32,
        /// The range's end frame number (exclusive).
        end: u32,
        /// The area's play time in frames.
        playtime: u32,
    },
    /// A caller-supplied argument is invalid (for example a buffer of the wrong size).
    #[error("invalid argument: {0}")]
    InvalidArgument(&'static str),
    /// The underlying reader failed.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
}

/// Broad classes of [`Error`], for callers (and the FFI) that map errors to codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorClass {
    /// The image was rejected (specification 7, 7.7.1).
    Rejected,
    /// A track was refused (no audio, an error).
    TrackRefused,
    /// A track was skipped (no audio, not an error).
    TrackSkipped,
    /// The caller asked for something that does not exist or passed a bad argument.
    Usage,
    /// The reader failed.
    Io,
}

impl Error {
    /// The diagnostic code of specification 7, if this error has one.
    pub fn code(&self) -> Option<&'static str> {
        Some(match self {
            Error::NotScarletbook => "not-scarletbook",
            Error::MasterTocTruncated => "master-toc-truncated",
            Error::MasterTocVersionUnsupported { .. } => "master-toc-version-unsupported",
            Error::NoAudioAreas => "no-audio-areas",
            Error::ChannelCountUnsupported { .. } => "channel-count-unsupported",
            Error::TrackEmpty => "track-empty",
            Error::TrackExceedsCapacity { .. } => "track-exceeds-capacity",
            Error::NoSuchArea(_)
            | Error::NoSuchTrack { .. }
            | Error::FrameRangeInvalid { .. }
            | Error::InvalidArgument(_)
            | Error::Io(_) => {
                return None;
            }
        })
    }

    /// The class of this error.
    pub fn class(&self) -> ErrorClass {
        match self {
            Error::NotScarletbook
            | Error::MasterTocTruncated
            | Error::MasterTocVersionUnsupported { .. }
            | Error::NoAudioAreas => ErrorClass::Rejected,
            Error::ChannelCountUnsupported { .. } | Error::TrackExceedsCapacity { .. } => {
                ErrorClass::TrackRefused
            }
            Error::TrackEmpty => ErrorClass::TrackSkipped,
            Error::NoSuchArea(_)
            | Error::NoSuchTrack { .. }
            | Error::FrameRangeInvalid { .. }
            | Error::InvalidArgument(_) => ErrorClass::Usage,
            Error::Io(_) => ErrorClass::Io,
        }
    }
}

/// The warning codes of specification 7, 7.7.2 and 7.7.3.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum WarningCode {
    /// No `SACDText` at LSN 511 and the text channel count is above 0.
    MasterTextMissing,
    /// No `SACD_Man` at LSN 519.
    ManufacturerMissing,
    /// Area TOC size above 128 sectors (area skipped).
    AreaTocTooLarge,
    /// TOC-1 cannot be read.
    AreaToc1Unreadable,
    /// TOC-2 is set but cannot be read.
    AreaToc2Unreadable,
    /// Both copies were read and differ.
    AreaTocCopiesDiffer,
    /// Neither TOC copy can be read (area skipped).
    AreaTocUnreadable,
    /// The chosen copy's signature is neither `TWOCHTOC` nor `MULCHTOC` (area skipped).
    AreaSignatureInvalid,
    /// No `SACDTRL1` or no `SACDTRL2` in the sub-structure walk (area skipped).
    AreaTrackListMissing,
    /// A `SACD_IGL` extends past the end of the area TOC (area skipped).
    AreaTocMalformed,
    /// The last audio LSN is below the first (area skipped).
    AreaAudioRangeReversed,
    /// A track text offset points too close to the end of the area TOC.
    TrackTextOffsetInvalid,
    /// A track text item of unknown type with a non-empty string.
    TrackTextTypeUnknown,
    /// A track ends after the area play time; it was clamped.
    TrackPastPlaytime,
    /// A track start LSN lies outside the area's audio range.
    TrackLsnOutsideArea,
    /// A malformed audio sector (optional).
    AudioSectorMalformed,
    /// A master TOC copy at LSN 520 or 530 differs from LSN 510 (optional; reported at
    /// most once, specification 1, 1.5.1 item 3).
    MasterTocCopiesDiffer,
    /// A sample frequency code other than 4 (optional).
    SampleFrequencyUnknown,
}

impl WarningCode {
    /// The diagnostic code string of specification 7.
    pub fn code(self) -> &'static str {
        match self {
            WarningCode::MasterTextMissing => "master-text-missing",
            WarningCode::ManufacturerMissing => "manufacturer-missing",
            WarningCode::AreaTocTooLarge => "area-toc-too-large",
            WarningCode::AreaToc1Unreadable => "area-toc1-unreadable",
            WarningCode::AreaToc2Unreadable => "area-toc2-unreadable",
            WarningCode::AreaTocCopiesDiffer => "area-toc-copies-differ",
            WarningCode::AreaTocUnreadable => "area-toc-unreadable",
            WarningCode::AreaSignatureInvalid => "area-signature-invalid",
            WarningCode::AreaTrackListMissing => "area-track-list-missing",
            WarningCode::AreaTocMalformed => "area-toc-malformed",
            WarningCode::AreaAudioRangeReversed => "area-audio-range-reversed",
            WarningCode::TrackTextOffsetInvalid => "track-text-offset-invalid",
            WarningCode::TrackTextTypeUnknown => "track-text-type-unknown",
            WarningCode::TrackPastPlaytime => "track-past-playtime",
            WarningCode::TrackLsnOutsideArea => "track-lsn-outside-area",
            WarningCode::AudioSectorMalformed => "audio-sector-malformed",
            WarningCode::MasterTocCopiesDiffer => "master-toc-copies-differ",
            WarningCode::SampleFrequencyUnknown => "sample-frequency-unknown",
        }
    }

    /// Whether the warning means the area was skipped (specification 7, 7.7.2).
    pub fn skips_area(self) -> bool {
        matches!(
            self,
            WarningCode::AreaTocTooLarge
                | WarningCode::AreaTocUnreadable
                | WarningCode::AreaSignatureInvalid
                | WarningCode::AreaTrackListMissing
                | WarningCode::AreaTocMalformed
                | WarningCode::AreaAudioRangeReversed
        )
    }
}

impl fmt::Display for WarningCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// A warning: a code with the slot, track and LSN it concerns, where they apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Warning {
    /// The diagnostic code.
    pub code: WarningCode,
    /// The area slot the warning concerns, if any.
    pub slot: Option<Slot>,
    /// The track number (1-based) the warning concerns, if any.
    pub track: Option<u8>,
    /// The LSN the warning concerns, if any (`audio-sector-malformed`).
    pub lsn: Option<u32>,
}

impl Warning {
    pub(crate) fn disc(code: WarningCode) -> Self {
        Warning { code, slot: None, track: None, lsn: None }
    }

    pub(crate) fn area(code: WarningCode, slot: Slot) -> Self {
        Warning { code, slot: Some(slot), track: None, lsn: None }
    }

    pub(crate) fn track(code: WarningCode, slot: Slot, track: u8) -> Self {
        Warning { code, slot: Some(slot), track: Some(track), lsn: None }
    }
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code.code())?;
        if let Some(slot) = self.slot {
            write!(f, " (area {slot}")?;
            if let Some(track) = self.track {
                write!(f, ", track {track}")?;
            }
            if let Some(lsn) = self.lsn {
                write!(f, ", LSN {lsn}")?;
            }
            f.write_str(")")?;
        }
        Ok(())
    }
}
