// SPDX-License-Identifier: MIT OR Apache-2.0

//! Stored sector formats, format detection and LSN addressing (specification 1).

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::control::CancelToken;
use crate::error::Error;
use crate::log::{Log, sb_log};
use crate::read_at::{ReadAt, ReadAtReader};

/// Size of a logical sector in bytes (specification 0, 0.3).
pub const SECTOR_SIZE: usize = 2048;

/// LSN of the first master TOC sector (specification 1, 1.5).
pub const MASTER_TOC_LSN: u32 = 510;

/// How logical sectors are stored in an image file (specification 1, 1.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SectorFormat {
    /// 2048 bytes per stored sector, data at offset 0.
    Plain,
    /// 2054 bytes per stored sector, data at offset 6.
    Raw2054,
    /// 2064 bytes per stored sector, data at offset 12.
    Raw2064,
}

impl SectorFormat {
    /// The detection order of specification 1, 1.4.
    pub const DETECTION_ORDER: [SectorFormat; 3] =
        [SectorFormat::Plain, SectorFormat::Raw2064, SectorFormat::Raw2054];

    /// Stored sector size `P` in bytes.
    pub fn stored_size(self) -> u64 {
        match self {
            SectorFormat::Plain => 2048,
            SectorFormat::Raw2054 => 2054,
            SectorFormat::Raw2064 => 2064,
        }
    }

    /// Offset `D` of the user data within a stored sector.
    pub fn data_offset(self) -> u64 {
        match self {
            SectorFormat::Plain => 0,
            SectorFormat::Raw2054 => 6,
            SectorFormat::Raw2064 => 12,
        }
    }
}

/// A disc image: a reader, its sector format and its sector count.
///
/// Reads follow specification 1, 1.3: a read of `n` sectors at `a` succeeds only if
/// `a + n <= N`, and a failing read returns no data.
#[derive(Debug)]
pub struct Image<R> {
    reader: R,
    id: u64,
    format: SectorFormat,
    len: u64,
    sectors: u64,
    /// Where the reader is positioned, if known, to avoid redundant seeks.
    pos: Option<u64>,
    scratch: Vec<u8>,
    log: Log,
    /// Shared with the images of the same source made from this one.
    cancel: CancelToken,
}

/// The outcome of a sector read that did not fail with an I/O error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadOutcome {
    /// All sectors existed and were copied.
    Ok,
    /// At least one sector does not exist; nothing was copied.
    OutOfRange,
}

impl<R: Read + Seek> Image<R> {
    /// Opens an image, detecting its sector format (specification 1, 1.4).
    ///
    /// # Errors
    ///
    /// [`Error::NotScarletbook`] if no format matches, [`Error::Io`] if the reader fails.
    pub fn open(reader: R) -> Result<Self, Error> {
        Self::open_with_log(reader, Log::none())
    }

    /// Opens an image, detecting its sector format, and reports to `log` (see
    /// [`crate::log`]) from the detection on.
    ///
    /// # Errors
    ///
    /// As [`Image::open`].
    pub fn open_with_log(mut reader: R, log: Log) -> Result<Self, Error> {
        let len = match reader.seek(SeekFrom::End(0)) {
            Ok(len) => len,
            Err(e) => {
                sb_log!(&log, Error, "image length: seek to the end failed: {e}");
                return Err(e.into());
            }
        };
        sb_log!(&log, Debug, "image length {len} bytes; probing sector formats");
        let mut sig = [0u8; 8];
        for format in SectorFormat::DETECTION_ORDER {
            // 510 x P + D + 8 is at most about 1 MB: no overflow.
            let at = u64::from(MASTER_TOC_LSN) * format.stored_size() + format.data_offset();
            if at + 8 > len {
                sb_log!(&log, Debug, "format {format:?}: signature bytes {at}..{} past the end", at + 8);
                continue;
            }
            if let Err(e) = reader.seek(SeekFrom::Start(at)).and_then(|_| reader.read_exact(&mut sig)) {
                sb_log!(&log, Error, "format {format:?}: read of bytes {at}..{} failed: {e}", at + 8);
                return Err(e.into());
            }
            if &sig == b"SACDMTOC" {
                let image = Self::new(reader, format, len, log);
                sb_log!(
                    &image.log,
                    Info,
                    "sector format {format:?} detected (signature at byte {at}): {} bytes per sector, data at \
                     offset {}, {} sectors",
                    format.stored_size(),
                    format.data_offset(),
                    image.sectors
                );
                return Ok(image);
            }
            sb_log!(&log, Debug, "format {format:?}: no signature at byte {at}");
        }
        sb_log!(&log, Info, "no sector format matches: not a Scarletbook image");
        Err(Error::NotScarletbook)
    }

    /// Opens an image with a sector format chosen by the caller (specification 1, 1.4
    /// item 5). The master TOC signature is then checked by the TOC parser.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] if the reader fails.
    pub fn with_format(reader: R, format: SectorFormat) -> Result<Self, Error> {
        Self::with_format_and_log(reader, format, Log::none())
    }

    /// Opens an image with a sector format chosen by the caller, and reports to `log`
    /// from the start (see [`crate::log`]).
    ///
    /// # Errors
    ///
    /// As [`Image::with_format`].
    pub fn with_format_and_log(mut reader: R, format: SectorFormat, log: Log) -> Result<Self, Error> {
        let len = match reader.seek(SeekFrom::End(0)) {
            Ok(len) => len,
            Err(e) => {
                sb_log!(&log, Error, "image length: seek to the end failed: {e}");
                return Err(e.into());
            }
        };
        let image = Self::new(reader, format, len, log);
        sb_log!(
            &image.log,
            Info,
            "sector format {format:?} chosen by the caller: {} bytes per sector, data at offset {}, {} sectors \
             ({len} bytes)",
            format.stored_size(),
            format.data_offset(),
            image.sectors
        );
        Ok(image)
    }

    fn new(reader: R, format: SectorFormat, len: u64, log: Log) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        Image {
            reader,
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            format,
            len,
            sectors: len / format.stored_size(),
            pos: None,
            scratch: Vec::new(),
            log,
            cancel: CancelToken::new(),
        }
    }

    /// The log this image reports to.
    pub fn log(&self) -> &Log {
        &self.log
    }

    /// Replaces the log this image reports to. Decoding jobs already started keep the log
    /// they started with.
    pub fn set_log(&mut self, log: Log) {
        self.log = log;
    }

    /// An identifier of this image, unique within the process except that the images of
    /// [`crate::Disc::share`] and of `clone` (for a [`ReadAtReader`]) have their original's:
    /// they read the same bytes. Audio cursors bind to the identifier of the image they
    /// first read and refuse an image with any other.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// The cancellation request of this image's reads (see [`CancelToken`]), shared with
    /// the images of [`crate::Disc::share`] and of `clone`.
    pub fn cancel_token(&self) -> CancelToken {
        self.cancel.clone()
    }

    /// [`crate::Error::Cancelled`] if cancellation of this image's reads is requested.
    #[inline]
    pub(crate) fn check_cancel(&self) -> Result<(), Error> {
        self.cancel.check()
    }

    /// The sector format.
    pub fn format(&self) -> SectorFormat {
        self.format
    }

    /// The number of logical sectors `N` (specification 1, 1.3).
    pub fn sector_count(&self) -> u64 {
        self.sectors
    }

    /// The image length in bytes.
    pub fn len(&self) -> u64 {
        self.len
    }

    /// Whether the image is empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The same image read through `reader`, which the caller checks holds the same bytes:
    /// this image's format, sector count and identifier, with a position and buffer of its
    /// own, reporting to `log`.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidArgument`] if `reader` has another length, [`Error::Io`] if finding
    /// its length fails.
    pub(crate) fn share<R2: Read + Seek>(&self, mut reader: R2, log: Log) -> Result<Image<R2>, Error> {
        let len = match reader.seek(SeekFrom::End(0)) {
            Ok(len) => len,
            Err(e) => {
                sb_log!(&log, Error, "shared reader: seek to the end failed: {e}");
                return Err(e.into());
            }
        };
        if len != self.len {
            sb_log!(&log, Info, "shared reader refused: {len} bytes, the image has {}", self.len);
            return Err(Error::InvalidArgument("shared reader of another image"));
        }
        Ok(self.with_reader(reader, log))
    }

    fn with_reader<R2>(&self, reader: R2, log: Log) -> Image<R2> {
        Image {
            reader,
            id: self.id,
            format: self.format,
            len: self.len,
            sectors: self.sectors,
            pos: None,
            scratch: Vec::new(),
            log,
            cancel: self.cancel.clone(),
        }
    }

    /// Consumes the image and returns the reader.
    pub fn into_inner(self) -> R {
        self.reader
    }

    /// Whether a read of `count` sectors at `lsn` would succeed (specification 1, 1.3).
    pub fn contains(&self, lsn: u64, count: u64) -> bool {
        lsn.checked_add(count).is_some_and(|end| end <= self.sectors)
    }

    /// Reads `buf.len() / 2048` logical sectors starting at `lsn` into `buf`.
    ///
    /// Returns `Ok(false)` and leaves `buf` unspecified if any of the sectors does not
    /// exist. `buf.len()` must be a non-zero multiple of 2048.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidArgument`] for a bad buffer length, [`Error::Io`] if the reader
    /// fails (including a reader that is shorter than it reported).
    pub fn read_sectors(&mut self, lsn: u64, buf: &mut [u8]) -> Result<bool, Error> {
        if buf.is_empty() || buf.len() % SECTOR_SIZE != 0 {
            return Err(Error::InvalidArgument("buffer length must be a non-zero multiple of 2048"));
        }
        Ok(self.read_into(lsn, buf)? == ReadOutcome::Ok)
    }

    pub(crate) fn read_into(&mut self, lsn: u64, buf: &mut [u8]) -> io::Result<ReadOutcome> {
        let count = (buf.len() / SECTOR_SIZE) as u64;
        if !self.contains(lsn, count) {
            sb_log!(
                &self.log,
                Debug,
                "read of {count} sectors at LSN {lsn}: past the last sector ({})",
                self.sectors
            );
            return Ok(ReadOutcome::OutOfRange);
        }
        let p = self.format.stored_size();
        let d = self.format.data_offset();
        // lsn + count <= N <= len / P, so lsn x P <= len: no overflow.
        let start = lsn * p;
        sb_log!(&self.log, Debug, "read {count} sectors at LSN {lsn}: bytes {start}..{}", start + count * p);
        let result = if self.format == SectorFormat::Plain {
            self.seek_to(start).and_then(|()| self.reader.read_exact(buf)).map(|()| start + buf.len() as u64)
        } else {
            self.read_raw(start, count, p, d, buf)
        };
        match result {
            Ok(end) => {
                self.pos = Some(end);
                Ok(ReadOutcome::Ok)
            }
            Err(e) => {
                sb_log!(&self.log, Error, "read of {count} sectors at LSN {lsn} (byte {start}) failed: {e}");
                self.pos = None;
                Err(e)
            }
        }
    }

    fn read_raw(&mut self, start: u64, count: u64, p: u64, d: u64, buf: &mut [u8]) -> io::Result<u64> {
        // Read whole stored sectors in bounded chunks and copy out the user data.
        const CHUNK_SECTORS: usize = 64;
        self.seek_to(start)?;
        let p_usize = p as usize;
        let d_usize = d as usize;
        let mut done = 0usize;
        let total = count as usize;
        while done < total {
            let n = CHUNK_SECTORS.min(total - done);
            self.scratch.resize(n * p_usize, 0);
            self.reader.read_exact(&mut self.scratch)?;
            for (i, stored) in self.scratch.chunks_exact(p_usize).enumerate() {
                let dst_start = (done + i) * SECTOR_SIZE;
                let (Some(dst), Some(src)) = (
                    buf.get_mut(dst_start..dst_start + SECTOR_SIZE),
                    stored.get(d_usize..d_usize + SECTOR_SIZE),
                ) else {
                    return Err(io::Error::other("internal sector copy out of range"));
                };
                dst.copy_from_slice(src);
            }
            done += n;
        }
        Ok(start + count * p)
    }

    fn seek_to(&mut self, at: u64) -> io::Result<()> {
        if self.pos != Some(at) {
            self.pos = None;
            self.reader.seek(SeekFrom::Start(at))?;
            self.pos = Some(at);
        }
        Ok(())
    }
}

/// Another reader of the same source, with a position and buffer of its own and the same
/// format, sector count, identifier and log: no I/O. Cheap when the source is (`&T`,
/// `Arc<T>`); see [`ReadAtReader`].
impl<A: ReadAt + Clone> Clone for Image<ReadAtReader<A>> {
    fn clone(&self) -> Self {
        self.with_reader(self.reader.clone(), self.log.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn image_with_sig(format: SectorFormat, sectors: u64) -> Vec<u8> {
        let p = format.stored_size() as usize;
        let mut v = vec![0u8; sectors as usize * p];
        let at = 510 * p + format.data_offset() as usize;
        if let Some(sig) = v.get_mut(at..at + 8) {
            sig.copy_from_slice(b"SACDMTOC");
        }
        v
    }

    #[test]
    fn detects_each_format() {
        for f in SectorFormat::DETECTION_ORDER {
            let img = Image::open(Cursor::new(image_with_sig(f, 520))).unwrap();
            assert_eq!(img.format(), f);
            assert_eq!(img.sector_count(), 520);
        }
    }

    #[test]
    fn detection_needs_only_eight_bytes() {
        let mut v = image_with_sig(SectorFormat::Plain, 511);
        v.truncate(510 * 2048 + 8);
        let img = Image::open(Cursor::new(v)).unwrap();
        assert_eq!(img.sector_count(), 510);
        let v = vec![0u8; 510 * 2048 + 7];
        assert!(matches!(Image::open(Cursor::new(v)), Err(Error::NotScarletbook)));
    }

    #[test]
    fn reads_raw_sectors_and_fails_whole_reads() {
        let f = SectorFormat::Raw2064;
        let mut v = image_with_sig(f, 3);
        // Put a marker in the user data of sector 2.
        let at = 2 * 2064 + 12;
        v[at] = 0xAB;
        v[at + 2047] = 0xCD;
        // Partial trailing sector does not count.
        v.extend_from_slice(&[0u8; 2063]);
        let mut img = Image::with_format(Cursor::new(v), f).unwrap();
        assert_eq!(img.sector_count(), 3);
        let mut buf = vec![0u8; 2048];
        assert!(img.read_sectors(2, &mut buf).unwrap());
        assert_eq!((buf[0], buf[2047]), (0xAB, 0xCD));
        let mut two = vec![0u8; 4096];
        assert!(!img.read_sectors(2, &mut two).unwrap());
        assert!(!img.read_sectors(u64::MAX, &mut buf).unwrap());
    }
}
