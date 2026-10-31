// SPDX-License-Identifier: MIT OR Apache-2.0

//! Positional reads: image sources that several readers, on several threads, read at once.
//!
//! A [`ReadAt`] source is read at explicit offsets through a shared reference, so it keeps
//! no position of its own. [`ReadAtReader`] gives one reader of such a source a position of
//! its own: it is an ordinary `Read + Seek` reader, so it opens a [`crate::Image`] or a
//! [`crate::Disc`] like any other, and readers of one shared source (an `Arc<File>`, an
//! `Arc<[u8]>`, a `&[u8]`) can be used on different threads at the same time.
//!
//! ```no_run
//! # fn main() -> Result<(), scarletbook::Error> {
//! use std::sync::Arc;
//! use scarletbook::{Disc, ReadAtReader};
//!
//! let file = Arc::new(std::fs::File::open("disc.iso")?);
//! let disc = Disc::open(ReadAtReader::new(Arc::clone(&file)))?;
//! // A second reader of the same file, with a position of its own.
//! let mut second = ReadAtReader::new(file);
//! # let _ = (&disc, &mut second);
//! # Ok(())
//! # }
//! ```
//!
//! Sources for which there is no positional read can be shared through [`Locked`], which
//! serialises their reads.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex, PoisonError};

/// A byte source read at explicit offsets through a shared reference.
///
/// Implementations may be called from several threads at once when the type is `Sync`;
/// each call is independent of the others.
pub trait ReadAt {
    /// Reads up to `buf.len()` bytes starting at byte `offset` into `buf` and returns how
    /// many were read: 0 at or past the end. As for [`Read::read`], fewer bytes than asked
    /// is not an error.
    ///
    /// # Errors
    ///
    /// Any I/O error of the source.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize>;

    /// The source's length in bytes.
    ///
    /// # Errors
    ///
    /// Any I/O error of the source.
    fn size(&self) -> io::Result<u64>;
}

/// Reads with `pread` on Unix and `seek_read` on Windows.
///
/// On Windows `seek_read` also moves the file's cursor, and [`ReadAt::size`] does on every
/// platform: do not mix positional reads and [`Read`] on the same `File`. On Windows, reads
/// through one shared handle may also be serialised by the system; readers that need
/// their reads to proceed in parallel there can open the file once each.
#[cfg(any(unix, windows))]
impl ReadAt for File {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        #[cfg(unix)]
        {
            std::os::unix::fs::FileExt::read_at(self, buf, offset)
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::FileExt::seek_read(self, buf, offset)
        }
    }

    fn size(&self) -> io::Result<u64> {
        // The end found by seeking, as for any reader opened by `Image::open`: a block
        // device reports a length of 0 in its metadata.
        let mut file = self;
        file.seek(SeekFrom::End(0))
    }
}

impl ReadAt for [u8] {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let start = usize::try_from(offset).map_or(self.len(), |o| o.min(self.len()));
        let src = self.get(start..).unwrap_or_default();
        let n = src.len().min(buf.len());
        if let (Some(dst), Some(src)) = (buf.get_mut(..n), src.get(..n)) {
            dst.copy_from_slice(src);
        }
        Ok(n)
    }

    fn size(&self) -> io::Result<u64> {
        Ok(self.len() as u64)
    }
}

impl ReadAt for Vec<u8> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        self.as_slice().read_at(offset, buf)
    }

    fn size(&self) -> io::Result<u64> {
        self.as_slice().size()
    }
}

impl<T: ReadAt + ?Sized> ReadAt for &T {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        (**self).read_at(offset, buf)
    }

    fn size(&self) -> io::Result<u64> {
        (**self).size()
    }
}

impl<T: ReadAt + ?Sized> ReadAt for Box<T> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        (**self).read_at(offset, buf)
    }

    fn size(&self) -> io::Result<u64> {
        (**self).size()
    }
}

impl<T: ReadAt + ?Sized> ReadAt for Arc<T> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        (**self).read_at(offset, buf)
    }

    fn size(&self) -> io::Result<u64> {
        (**self).size()
    }
}

/// A `Read + Seek` reader shared through a lock: a [`ReadAt`] source for readers that have
/// no positional read. Reads are serialised, each seeking first unless the reader is
/// already at its offset.
#[derive(Debug)]
pub struct Locked<R> {
    inner: Mutex<LockedState<R>>,
}

#[derive(Debug)]
struct LockedState<R> {
    reader: R,
    /// Where the reader is positioned, if known, to avoid redundant seeks.
    pos: Option<u64>,
}

impl<R> Locked<R> {
    /// Shares `reader`.
    pub fn new(reader: R) -> Self {
        Locked { inner: Mutex::new(LockedState { reader, pos: None }) }
    }

    /// Consumes the lock and returns the reader.
    pub fn into_inner(self) -> R {
        self.inner.into_inner().unwrap_or_else(PoisonError::into_inner).reader
    }
}

impl<R: Read + Seek> ReadAt for Locked<R> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        // A reader that panicked left at worst an unknown position, which is reset below.
        let mut state = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let LockedState { reader, pos } = &mut *state;
        if *pos != Some(offset) {
            *pos = None;
            reader.seek(SeekFrom::Start(offset))?;
            *pos = Some(offset);
        }
        let n = reader.read(buf);
        *pos = match &n {
            Ok(n) => offset.checked_add(*n as u64),
            Err(_) => None,
        };
        n
    }

    fn size(&self) -> io::Result<u64> {
        let mut state = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        state.pos = None;
        let len = state.reader.seek(SeekFrom::End(0))?;
        state.pos = Some(len);
        Ok(len)
    }
}

/// A `Read + Seek` reader of a [`ReadAt`] source, with a position of its own.
///
/// Readers of one source do not move each other: each reads where its own position is.
/// Cloning one gives a reader with an independent position over the same source; that is
/// cheap when the source is (`&T`, `Arc<T>`), while a reader of a `Vec<u8>` clones the bytes
/// (share an `Arc<Vec<u8>>` or `Arc<[u8]>` instead).
#[derive(Clone, Debug)]
pub struct ReadAtReader<A> {
    source: A,
    pos: u64,
}

impl<A> ReadAtReader<A> {
    /// A reader of `source` at offset 0.
    pub fn new(source: A) -> Self {
        ReadAtReader { source, pos: 0 }
    }

    /// The source.
    pub fn get_ref(&self) -> &A {
        &self.source
    }

    /// The reader's position.
    pub fn position(&self) -> u64 {
        self.pos
    }

    /// Consumes the reader and returns the source.
    pub fn into_inner(self) -> A {
        self.source
    }
}

impl<A: ReadAt> Read for ReadAtReader<A> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let n = self.source.read_at(self.pos, buf)?;
        if n > buf.len() {
            return Err(io::Error::other("positional read returned more bytes than asked"));
        }
        // The source has bytes up to pos + n, so this does not overflow for a real source;
        // a source that claims otherwise is refused rather than wrapped.
        self.pos = self
            .pos
            .checked_add(n as u64)
            .ok_or_else(|| io::Error::other("positional read past the largest offset"))?;
        Ok(n)
    }
}

impl<A: ReadAt> Seek for ReadAtReader<A> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let (base, offset) = match pos {
            SeekFrom::Start(n) => {
                self.pos = n;
                return Ok(n);
            }
            SeekFrom::End(n) => (self.source.size()?, n),
            SeekFrom::Current(n) => (self.pos, n),
        };
        let new = base.checked_add_signed(offset).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "invalid seek to a negative or overflowing position")
        })?;
        self.pos = new;
        Ok(new)
    }

    fn stream_position(&mut self) -> io::Result<u64> {
        Ok(self.pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn slice_reads_and_ends() {
        let data: Vec<u8> = (0..10).collect();
        let mut buf = [0u8; 4];
        assert_eq!(data.read_at(8, &mut buf).unwrap(), 2);
        assert_eq!(&buf[..2], &[8, 9]);
        assert_eq!(data.read_at(10, &mut buf).unwrap(), 0);
        assert_eq!(data.read_at(u64::MAX, &mut buf).unwrap(), 0);
        assert_eq!(data.size().unwrap(), 10);
    }

    #[test]
    fn readers_of_one_source_keep_their_own_positions() {
        let data: Arc<[u8]> = (0..=255).collect();
        let mut a = ReadAtReader::new(Arc::clone(&data));
        let mut b = a.clone();
        a.seek(SeekFrom::Start(200)).unwrap();
        let (mut x, mut y) = ([0u8; 3], [0u8; 3]);
        a.read_exact(&mut x).unwrap();
        b.read_exact(&mut y).unwrap();
        assert_eq!((x, y), ([200, 201, 202], [0, 1, 2]));
        assert_eq!((a.position(), b.position()), (203, 3));
    }

    #[test]
    fn seeks_as_a_cursor_does() {
        let data = [7u8; 100];
        let mut r = ReadAtReader::new(&data[..]);
        let mut c = Cursor::new(&data[..]);
        for p in [
            SeekFrom::End(0),
            SeekFrom::End(-10),
            SeekFrom::End(5),
            SeekFrom::Current(-200),
            SeekFrom::Current(3),
            SeekFrom::Start(u64::MAX),
            SeekFrom::Current(1),
            SeekFrom::End(-101),
        ] {
            let (x, y) = (r.seek(p), c.seek(p));
            assert_eq!(x.as_ref().ok(), y.as_ref().ok(), "{p:?}");
            assert_eq!(x.err().map(|e| e.kind()), y.err().map(|e| e.kind()), "{p:?}");
        }
    }

    #[test]
    fn locked_reads_and_reseeks() {
        let shared = Locked::new(Cursor::new((0..100).collect::<Vec<u8>>()));
        let mut buf = [0u8; 2];
        assert_eq!(shared.read_at(50, &mut buf).unwrap(), 2);
        assert_eq!(buf, [50, 51]);
        assert_eq!(shared.size().unwrap(), 100);
        assert_eq!(shared.read_at(98, &mut buf).unwrap(), 2);
        assert_eq!(buf, [98, 99]);
        assert_eq!(shared.read_at(100, &mut buf).unwrap(), 0);
        assert_eq!(shared.into_inner().position(), 100);
    }
}
