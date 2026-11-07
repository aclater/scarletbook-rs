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
//!
//! # Threads
//!
//! Every public type of this crate is [`Send`] and [`Sync`], except that those holding a
//! reader follow the reader:
//!
//! | Type | `Send` if | `Sync` if |
//! |---|---|---|
//! | [`Disc<R>`], [`Image<R>`], [`AreaFrames<R>`], [`TrackFrames<R>`], [`TrackAudio<R>`] | `R: Send` | `R: Sync` |
//! | [`ReadAtReader<A>`] | `A: Send` | `A: Sync` |
//! | [`Locked<R>`](read_at::Locked) | `R: Send` | `R: Send` |
//!
//! This is part of the API, not an accident of the fields: assertions in this crate's
//! source stop it building if a type loses one of these. It is a statement about what the
//! compiler allows, so here is what it means for reading:
//!
//! - **Reading is one thread per [`Disc`].** A disc has one reader with one position, so
//!   its reads take `&mut self`. Its `&self` methods (the TOC, making a cursor) may run on
//!   several threads at once when `R: Sync`.
//! - **To read one image on several threads at once,** give each a disc of its own
//!   ([`Disc::share`], or `clone` of a disc read through a [`ReadAtReader`]): see
//!   [Several readers](Disc#several-readers). A [`ReadAt`] source is called from several
//!   threads at once when it is `Sync`.
//! - **Cursors hold no reader.** [`TrackAudioCursor`], [`TrackCursor`] and [`AreaCursor`]
//!   can move between threads, and be read through any disc of the same image. A
//!   borrowing iterator can move only with the `&mut` disc it borrows.
//! - **Results own their bytes.** [`TrackFrame`], [`AudioFrame`] and [`AreaFrame`] borrow
//!   nothing, so a frame can go to a worker thread and its decoded result come back.
//! - **A [`DstDecoder`] decodes one frame at a time** (`&mut self`): one per worker. It holds
//!   only buffers, so it can also move between threads.
//! - **Made to be used while another thread reads:** [`CancelToken`], [`Progress`], and the
//!   per-area seek index that a disc and the discs made from it share.
//! - **A [`Log`](log::Log) sink must be `Send + Sync`,** since with the `parallel` feature it
//!   is also called from the threads that decode.
//!
//! The crate uses no thread of its own without the `parallel` feature. Its global state
//! carries nothing from one image to another: atomic counters that number images and
//! provisional readings, the charset tables, built once on first use, and with `parallel`,
//! a decoder cached on each thread that decodes. With `parallel`, a cursor not given a pool
//! of its own (`DecodeOn`) uses rayon's current pool, by default its global one.
//!
//! **Stack.** Calls may use up to 68 KiB of the calling thread's stack (measured on x86-64
//! Linux in release builds; `Layout::read_at` of the `output` module, over a DST area on a
//! CPU with AVX2, is the deepest), and the reader, [`ReadAt`] source or [`Log`](log::Log)
//! sink they call runs on top of that: give threads that call this crate at least 136 KiB. The
//! threads that decode with `parallel` use about 20 KiB of the 2 MiB that rayon gives them
//! (Rust's default, or `RUST_MIN_STACK`). Unoptimised builds of this crate need far more:
//! on a CPU with AVX2, decoding a DST frame then takes about 2.1 MiB and `Layout::read_at`
//! about 2.5 MiB, more than the 2 MiB of a new Rust thread, so a debug build that decodes
//! on rayon's threads overflows their stacks. Build this crate optimised in the
//! debug profile (`[profile.dev.package.scarletbook] opt-level = 2`, as this repository
//! does), or set `RUST_MIN_STACK`.

// `unsafe` is denied everywhere except in `dispatch`, which calls the AVX2 build of the
// DST decoder and of the output path after checking for its features (CONTRIBUTING).
#![deny(unsafe_code)]
#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

// The test encoder shared with the integration tests names the crate (`dispatch` tests).
#[cfg(test)]
extern crate self as scarletbook;

pub mod audio;
mod control;
mod disc;
mod dispatch;
pub mod dst;
mod error;
mod image;
pub mod log;
#[cfg(feature = "output")]
pub mod output;
pub mod read_at;
pub mod seek;
pub mod text;
pub mod toc;

#[cfg(feature = "parallel")]
pub use audio::DecodeOn;
pub use audio::{
    AreaCursor, AreaFrame, AreaFrames, AudioFrame, Continuation, CursorOptions, Demuxer, FrameContent,
    FrameKind, FrameSpan, FrameStatus, Opening, Stored, TrackAudio, TrackAudioCursor, TrackCursor,
    TrackFrame, TrackFrames,
};
pub use control::{CancelToken, Progress};
pub use disc::{Disc, DiscOptions};
pub use dst::{DstDecoder, DstError};
pub use error::{EditedMasterCheck, Error, ErrorClass, OutputRefusal, Slot, Warning, WarningCode};
pub use image::{Image, ImageOptions, MASTER_TOC_LSN, SECTOR_SIZE, SectorFormat};
pub use read_at::{ReadAt, ReadAtReader};
pub use seek::{
    AreaIndex, IndexOptions, ResumedFrom, SeekEvent, SeekPolicy, Speculation, SpeculativeOptions, Suspect,
};
pub use toc::{Area, AreaKind, LoudspeakerLayout, MasterToc, Speaker, Toc, Track};

// The guarantees of "Threads" above: losing one of these fails the build (#39).
const _: () = {
    const fn send<T: Send>() {}
    const fn sync<T: Sync>() {}
    const fn both<T: Send + Sync>() {}

    // These hold for every reader with the bound, not only the ones named below.
    const fn with_send_reader<'a, R: Send + 'a>() {
        send::<Disc<R>>();
        send::<Image<R>>();
        send::<AreaFrames<'a, R>>();
        send::<TrackFrames<'a, R>>();
        send::<TrackAudio<'a, R>>();
        send::<ReadAtReader<R>>();
        both::<read_at::Locked<R>>();
    }
    const fn with_sync_reader<'a, R: Sync + 'a>() {
        sync::<Disc<R>>();
        sync::<Image<R>>();
        sync::<AreaFrames<'a, R>>();
        sync::<TrackFrames<'a, R>>();
        sync::<TrackAudio<'a, R>>();
        sync::<ReadAtReader<R>>();
    }
    with_send_reader::<std::fs::File>();
    with_sync_reader::<std::fs::File>();

    // Reading.
    both::<AreaCursor>();
    both::<TrackCursor>();
    both::<TrackAudioCursor>();
    both::<Continuation<'_>>();
    both::<Opening>();
    both::<Demuxer>();
    both::<DstDecoder>();
    both::<AreaFrame>();
    both::<TrackFrame>();
    both::<AudioFrame>();
    both::<FrameContent>();
    both::<FrameKind>();
    both::<FrameStatus>();
    both::<Stored>();
    both::<SectorFormat>();
    both::<DiscOptions>();
    both::<ImageOptions>();
    both::<CancelToken>();
    both::<Progress>();
    both::<log::Log>();
    both::<log::Level>();
    both::<text::Charset>();
    #[cfg(feature = "parallel")]
    both::<DecodeOn>();

    // Errors and warnings.
    both::<Error>();
    both::<DstError>();
    both::<ErrorClass>();
    both::<OutputRefusal>();
    both::<EditedMasterCheck>();
    both::<Slot>();
    both::<Warning>();
    both::<WarningCode>();

    // The TOC.
    both::<Toc>();
    both::<MasterToc>();
    both::<Area>();
    both::<Track>();
    both::<AreaKind>();
    both::<LoudspeakerLayout>();
    both::<Speaker>();
    both::<toc::Version>();
    both::<toc::Genre>();
    both::<toc::Date>();
    both::<toc::Locale>();
    both::<toc::Timecode>();
    both::<toc::AreaPointer>();
    both::<toc::DiscText>();
    both::<toc::AreaText>();
    both::<toc::TrackText>();
    both::<toc::DiscTextItem>();
    both::<toc::AreaTextItem>();
    both::<toc::TrackTextItem>();

    // Seeking.
    both::<AreaIndex>();
    both::<IndexOptions>();
    both::<ResumedFrom>();
    both::<SeekEvent>();
    both::<SeekPolicy>();
    both::<Speculation>();
    both::<SpeculativeOptions>();
    both::<Suspect>();

    // Output.
    #[cfg(feature = "output")]
    {
        both::<output::OutputOptions>();
        both::<output::Layout>();
        both::<output::Region>();
        both::<output::RegionKind>();
        both::<output::Input>();
        both::<output::Format>();
        both::<output::SourceTrack>();
        both::<output::Tags>();
        both::<output::TagSet>();
        both::<output::Id3Options>();
        both::<output::GenreSources>();
        both::<output::Marker>();
        both::<output::Comment>();
        both::<output::CallerChunk>();
        both::<output::DsfChainFile>();
        both::<output::DstLengths>();
        both::<output::AlbumArtistFrame>();
        both::<output::ArrangerFrame>();
        both::<output::ArtistTitleSource>();
        both::<output::CommentKind>();
        both::<output::DateFrame>();
        both::<output::DsdiffDeparture>();
        both::<output::DsfBitsPerSample>();
        both::<output::DstIndexOffset>();
        both::<output::Id3Encoding>();
        both::<output::InfoChunk>();
        both::<output::InvalidDst>();
        both::<output::LocalChunk>();
        both::<output::MarkerKind>();
        both::<output::MarkerText>();
        both::<output::Markers>();
        both::<output::PropertyChunk>();
        both::<output::TagItem>();
        both::<output::TextSource>();
    }
};
