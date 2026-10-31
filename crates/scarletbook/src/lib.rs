// SPDX-License-Identifier: MIT OR Apache-2.0

//! A clean-room reader for Super Audio CD (Scarletbook) disc images.
//!
//! Written only from the specifications of `aclater/sacd-spec`; section numbers in the
//! documentation refer to them. Every value read from an image is treated as hostile:
//! no input makes this crate panic, loop without bound or allocate without bound.
//!
//! ```no_run
//! # fn main() -> Result<(), scarletbook::Error> {
//! let file = std::fs::File::open("disc.iso")?;
//! let disc = scarletbook::Disc::open(std::io::BufReader::new(file))?;
//! for area in &disc.toc().areas {
//!     println!("{}: {} tracks", area.slot, area.tracks.len());
//! }
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod audio;
mod disc;
pub mod dst;
mod error;
mod image;
pub mod log;
pub mod seek;
pub mod text;
pub mod toc;

#[cfg(feature = "parallel")]
pub use audio::DecodeOn;
pub use audio::{
    AreaCursor, AreaFrame, AreaFrames, AudioFrame, Continuation, Demuxer, FrameContent, FrameKind,
    FrameStatus, TrackAudio, TrackAudioCursor, TrackCursor, TrackFrame, TrackFrames,
};
pub use disc::Disc;
pub use dst::{DstDecoder, DstError};
pub use error::{Error, ErrorClass, Slot, Warning, WarningCode};
pub use image::{Image, MASTER_TOC_LSN, SECTOR_SIZE, SectorFormat};
pub use seek::{AreaIndex, IndexOptions, ResumedFrom};
pub use toc::{Area, AreaKind, LoudspeakerLayout, MasterToc, Speaker, Toc, Track};
