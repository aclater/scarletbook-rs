// SPDX-License-Identifier: MIT OR Apache-2.0

//! Logical sector reads through the C ABI (`sb_disc_read_sectors`, issue #105), driven from
//! Rust as a C caller would.
//!
//! The expected bytes never come from the library: for an image of 2048-byte sectors they
//! are the file's bytes, and for 2054- and 2064-byte images a copy stripped here, from the
//! data offsets of specification 1, 1.2.
//!
//! Each unsafe call sits in a small wrapper that states why its arguments meet the
//! function's contract; the tests themselves are safe code.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::ffi::{CStr, CString, c_char, c_void};
use std::path::PathBuf;
use std::ptr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use scarletbook_ffi::*;

const S: usize = SB_SECTOR_SIZE as usize;

fn corpus_images() -> Vec<(String, PathBuf)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/images");
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "iso"))
        .map(|p| (p.file_stem().unwrap().to_string_lossy().into_owned(), p))
        .collect();
    v.sort();
    v
}

fn image(id: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../corpus/images/{id}.iso"));
    std::fs::read(path).unwrap()
}

/// The user data of every whole stored sector of `stored`, with `p` bytes per stored sector
/// and the data at offset `d` (specification 1, 1.2: 2048/0, 2054/6, 2064/12).
fn strip(stored: &[u8], p: usize) -> Vec<u8> {
    let d = match p {
        2048 => 0,
        2054 => 6,
        2064 => 12,
        _ => panic!("stored sector size {p}"),
    };
    stored.chunks_exact(p).flat_map(|s| &s[d..d + S]).copied().collect()
}

/// `plain` (2048-byte sectors) stored with `p`-byte sectors, the bytes around the user data
/// filled with 0x5A (as #74's tests build converted images).
fn reformat(plain: &[u8], p: usize) -> Vec<u8> {
    let d = if p == 2054 { 6 } else { 12 };
    let mut out = Vec::with_capacity(plain.len() / S * p);
    for sector in plain.chunks_exact(S) {
        let at = out.len();
        out.resize(at + p, 0x5A);
        out[at + d..at + d + S].copy_from_slice(sector);
    }
    out
}

fn open(bytes: &[u8]) -> Result<*mut SbDisc, SbStatus> {
    let mut d: *mut SbDisc = ptr::dangling_mut();
    // SAFETY: `bytes` is valid for its length; `&mut d` is a valid out-pointer.
    match unsafe { sb_disc_open_memory(bytes.as_ptr(), bytes.len(), &mut d) } {
        SbStatus::Ok => Ok(d),
        s => Err(s),
    }
}

fn clone(disc: *const SbDisc) -> *mut SbDisc {
    let mut d: *mut SbDisc = ptr::null_mut();
    // SAFETY: `disc` is a live handle not used by another thread during the call; `&mut d`
    // is a valid out-pointer.
    assert_eq!(unsafe { sb_disc_clone(disc, &mut d) }, SbStatus::Ok);
    d
}

fn free(disc: *mut SbDisc) {
    // SAFETY: `disc` is NULL or a live handle that the caller does not use again.
    unsafe { sb_disc_free(disc) };
}

fn info(disc: *const SbDisc) -> SbDiscInfo {
    let mut i = SbDiscInfo::default();
    // SAFETY: `disc` is a live handle; `&mut i` is a valid out-pointer.
    assert_eq!(unsafe { sb_disc_info(disc, &mut i) }, SbStatus::Ok);
    i
}

/// Reads `count` sectors at `lsn` into `buf` (as its capacity).
fn read_into(disc: *const SbDisc, lsn: u64, count: u32, buf: &mut [u8]) -> SbStatus {
    // SAFETY: `disc` is NULL or a live handle; `buf` is valid for its length.
    unsafe { sb_disc_read_sectors(disc, lsn, count, buf.as_mut_ptr(), buf.len()) }
}

/// Whether `b` still holds the 0xC3 a buffer is filled with (compared whole: fast in Miri).
fn untouched(b: &[u8]) -> bool {
    b == vec![0xC3u8; b.len()]
}

/// Reads `count` sectors at `lsn`, checking that nothing is written past them.
fn read(disc: *const SbDisc, lsn: u64, count: u32) -> Result<Vec<u8>, SbStatus> {
    let n = count as usize * S;
    let mut buf = vec![0xC3u8; n + 64];
    let s = read_into(disc, lsn, count, &mut buf);
    assert!(untouched(&buf[n..]), "written past {count} sectors at {lsn}");
    match s {
        SbStatus::Ok => {
            buf.truncate(n);
            Ok(buf)
        }
        s => {
            assert!(untouched(&buf), "a failed read at {lsn} wrote");
            Err(s)
        }
    }
}

fn last_error() -> String {
    // SAFETY: a valid NUL-terminated string until this thread's next call.
    unsafe { CStr::from_ptr(sb_last_error_message()) }.to_string_lossy().into_owned()
}

/// Every LSN of `disc`, one at a time and in one read, equals `user` (the image's user
/// data); reads past the end fail with nothing written.
fn check_every_lsn(disc: *const SbDisc, user: &[u8], what: &str, every: usize) {
    let n = user.len() / S;
    assert_eq!(info(disc).sectors, n as u64, "{what}");
    assert!(read(disc, 0, n as u32).unwrap() == user, "{what}: the whole image in one read");
    for lsn in (0..n).step_by(every) {
        let got = read(disc, lsn as u64, 1).unwrap_or_else(|s| panic!("{what}: LSN {lsn}: {s:?}"));
        assert!(got == user[lsn * S..][..S], "{what}: LSN {lsn}");
    }
    for (lsn, count) in [(n as u64, 1), (n as u64 - 1, 2), (0, n as u32 + 1), (u64::MAX, 1), (u64::MAX, 0)] {
        assert_eq!(read(disc, lsn, count), Err(SbStatus::SectorRangeInvalid), "{what}: {count} at {lsn}");
    }
    assert_eq!(read(disc, n as u64, 0), Ok(Vec::new()), "{what}");
}

/// For every corpus image and every LSN, the bytes read equal the image's user data: the
/// file's bytes for 2048-byte sectors, an independently stripped copy for 2054 and 2064.
#[test]
#[cfg_attr(miri, ignore = "reads every corpus image")]
fn every_lsn_of_every_corpus_image() {
    let mut formats = [0usize; 3];
    for (id, path) in corpus_images() {
        let bytes = std::fs::read(&path).unwrap();
        let Ok(disc) = open(&bytes) else { continue };
        let p = info(disc).sector_size as usize;
        formats[[2048, 2054, 2064].iter().position(|&x| x == p).unwrap()] += 1;
        let user = strip(&bytes, p);
        if p == S {
            assert!(user[..] == bytes[..user.len()]);
        }
        check_every_lsn(disc, &user, &id, 1);
        free(disc);
    }
    assert!(formats.iter().all(|&n| n > 0), "every stored format is in the corpus: {formats:?}");
}

/// Images of 2048-byte sectors converted to 2054 and 2064: every LSN reads the original
/// sector.
#[test]
#[cfg_attr(miri, ignore = "reads corpus images")]
fn every_lsn_of_converted_corpus_images() {
    for (id, path) in corpus_images() {
        let bytes = std::fs::read(&path).unwrap();
        let Ok(disc) = open(&bytes) else { continue };
        let plain = info(disc).sector_size == SB_SECTOR_SIZE;
        free(disc);
        if !plain || !(id.starts_with("dst-") || id.starts_with("dsd-") || id.starts_with("audio-")) {
            continue;
        }
        let user = &bytes[..bytes.len() / S * S];
        for p in [2054, 2064] {
            let converted = reformat(user, p);
            let disc = open(&converted).unwrap();
            assert_eq!(info(disc).sector_size as usize, p);
            check_every_lsn(disc, user, &format!("{id} as {p}"), 1);
            free(disc);
        }
    }
}

/// A positional read callback over an image, counting its calls.
struct Counted {
    data: Vec<u8>,
    calls: AtomicUsize,
}

unsafe extern "C" fn cb_counted_at(user: *mut c_void, offset: u64, buf: *mut u8, len: usize) -> i64 {
    // SAFETY: `user` is a `Counted` that outlives the disc and its clones; it is only read.
    let u = unsafe { &*user.cast::<Counted>() };
    u.calls.fetch_add(1, Ordering::SeqCst);
    let start = (offset as usize).min(u.data.len());
    let n = len.min(u.data.len() - start);
    // SAFETY: the library passes a buffer valid for `len >= n` bytes of writes.
    unsafe { ptr::copy_nonoverlapping(u.data[start..].as_ptr(), buf, n) };
    n as i64
}

fn open_counted(user: &Counted) -> *mut SbDisc {
    let mut d: *mut SbDisc = ptr::null_mut();
    let u = ptr::from_ref(user).cast_mut().cast::<c_void>();
    // SAFETY: `cb_counted_at` may be called with `u` from any thread, concurrently, while
    // `user` lives, which outlives the disc and its clones; `&mut d` is a valid out-pointer.
    let s = unsafe { sb_disc_open_callbacks_at(Some(cb_counted_at), user.data.len() as u64, u, &mut d) };
    assert_eq!(s, SbStatus::Ok);
    d
}

/// Large reads are a few source reads, not one per sector: one for 2048-byte sectors, one
/// per 64 stored sectors for the others.
#[test]
#[cfg_attr(miri, ignore = "reads a corpus image")]
fn large_reads_are_few_source_reads() {
    let plain = image("dst-features-2ch-6ch");
    let plain = &plain[..plain.len() / S * S];
    assert!(plain.len() / S >= 600);
    for (p, most) in [(2048, 1), (2054, 8), (2064, 8)] {
        let data = if p == 2048 { plain.to_vec() } else { reformat(plain, p) };
        let user = Counted { data, calls: AtomicUsize::new(0) };
        let disc = open_counted(&user);
        let copy = clone(disc);
        for d in [disc, copy] {
            for lsn in [0, 37] {
                user.calls.store(0, Ordering::SeqCst);
                assert!(read(d, lsn, 512).unwrap() == plain[lsn as usize * S..][..512 * S], "{p} at {lsn}");
                let calls = user.calls.load(Ordering::SeqCst);
                assert!((1..=most).contains(&calls), "{p}: 512 sectors at {lsn} took {calls} reads");
            }
        }
        free(copy);
        free(disc);
    }
}

/// Clones of file, memory and callback discs read sectors on several threads at once.
#[test]
#[cfg_attr(miri, ignore = "reads corpus images")]
fn clones_read_sectors_on_threads() {
    const THREADS: usize = 4;
    let id = "dsd-stereo-mch-2064";
    let bytes = image(id);
    let user = strip(&bytes, 2064);
    let path = CString::new(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../corpus/images/{id}.iso"))
            .to_str()
            .unwrap(),
    )
    .unwrap();
    let mut file: *mut SbDisc = ptr::null_mut();
    // SAFETY: `path` is a valid NUL-terminated string; `&mut file` a valid out-pointer.
    assert_eq!(unsafe { sb_disc_open_file(path.as_ptr(), &mut file) }, SbStatus::Ok);
    let counted = Counted { data: bytes.clone(), calls: AtomicUsize::new(0) };
    for disc in [file, open(&bytes).unwrap(), open_counted(&counted)] {
        let clones: Vec<usize> = (0..THREADS).map(|_| clone(disc).expose_provenance()).collect();
        let n = user.len() / S;
        std::thread::scope(|s| {
            for (i, &c) in clones.iter().enumerate() {
                let user = &user;
                s.spawn(move || {
                    let c: *const SbDisc = ptr::with_exposed_provenance(c);
                    for round in 0..3 {
                        for lsn in (i..n).step_by(THREADS * 7) {
                            let count = (1 + (lsn + round) % 70).min(n - lsn);
                            let got = read(c, lsn as u64, count as u32).unwrap();
                            assert!(got == user[lsn * S..][..count * S], "clone {i}: {count} at {lsn}");
                        }
                    }
                });
            }
        });
        clones.into_iter().map(ptr::with_exposed_provenance_mut).for_each(free);
        free(disc);
    }
}

/// The lines a log callback received.
struct Lines(Mutex<Vec<(i32, String)>>);

unsafe extern "C" fn cb_log(user: *mut c_void, level: i32, message: *const c_char) {
    // SAFETY: `user` is the `Lines` given with this callback, alive while the disc is.
    let user = unsafe { &*user.cast::<Lines>() };
    // SAFETY: the library passes a valid NUL-terminated string for the call.
    let m = unsafe { CStr::from_ptr(message) }.to_string_lossy().into_owned();
    user.0.lock().unwrap().push((level, m));
}

/// The tiny disc of the `ffi` tests (one area, no audio), its unused sectors filled with a pattern: small enough
/// for Miri.
fn tiny_image() -> Vec<u8> {
    let s = S;
    let mut img = vec![0u8; 547 * s];
    let pattern: Vec<u8> = (0..s).map(|j| (j % 251) as u8).collect();
    for (i, sector) in img[..510 * s].chunks_exact_mut(s).enumerate() {
        sector.copy_from_slice(&pattern);
        sector[..2].copy_from_slice(&(i as u16).to_be_bytes());
    }
    let m = 510 * s;
    img[m..m + 8].copy_from_slice(b"SACDMTOC");
    img[m + 8] = 1;
    img[m + 24..m + 29].copy_from_slice(b"CAT-1");
    img[m + 64..m + 68].copy_from_slice(&540u32.to_be_bytes());
    img[m + 84..m + 86].copy_from_slice(&6u16.to_be_bytes());
    img[m + 9 * s..m + 9 * s + 8].copy_from_slice(b"SACD_Man");
    let a = 540 * s;
    img[a..a + 8].copy_from_slice(b"TWOCHTOC");
    img[a + 32] = 2;
    img[a + 66] = 10;
    img[a + 69] = 1;
    img[a + 72..a + 76].copy_from_slice(&546u32.to_be_bytes());
    img[a + 76..a + 80].copy_from_slice(&546u32.to_be_bytes());
    img[a + s..a + s + 8].copy_from_slice(b"SACDTRL1");
    let t2 = a + 2 * s;
    img[t2..t2 + 8].copy_from_slice(b"SACDTRL2");
    img[t2 + 1028 + 2] = 2;
    img
}

/// Reads of the tiny image in each stored format, its arguments, and the log.
#[test]
fn sector_reads_of_a_tiny_image() {
    let plain = tiny_image();
    // Miri: the raw formats share their code; one of them is enough.
    let (every, formats): (_, &[usize]) =
        if cfg!(miri) { (61, &[2048, 2064]) } else { (1, &[2048, 2054, 2064]) };
    for &p in formats {
        let stored = if p == 2048 { plain.clone() } else { reformat(&plain, p) };
        let disc = open(&stored).unwrap();
        assert_eq!(info(disc).sector_size as usize, p);
        check_every_lsn(disc, &plain, &format!("tiny {p}"), every);
        assert_eq!(read(disc, 546, 2), Err(SbStatus::SectorRangeInvalid));
        assert_eq!(last_error(), "sectors 546..548 are not all in the image's 547 sectors");
        // A clone, on this thread.
        let copy = clone(disc);
        assert!(read(copy, 509, 2).unwrap() == plain[509 * S..511 * S]);
        free(copy);
        free(disc);
    }
    let disc = open(&plain).unwrap();
    // Buffers: NULL or too small write nothing; count 0 needs none.
    let mut small = vec![0xC3u8; 2 * S - 1];
    assert_eq!(read_into(disc, 0, 2, &mut small), SbStatus::BufferTooSmall);
    assert!(untouched(&small));
    assert_eq!(read_into(disc, 0, 1, &mut small), SbStatus::Ok);
    assert!(small[..S] == plain[..S] && untouched(&small[S..]));
    // SAFETY: a NULL buffer is allowed.
    assert_eq!(unsafe { sb_disc_read_sectors(disc, 0, 1, ptr::null_mut(), 4096) }, SbStatus::BufferTooSmall);
    // SAFETY: as above.
    assert_eq!(unsafe { sb_disc_read_sectors(disc, 3, 0, ptr::null_mut(), 0) }, SbStatus::Ok);
    // A range check comes first, also for counts whose bytes do not fit a size_t.
    assert_eq!(read_into(disc, 0, u32::MAX, &mut small), SbStatus::SectorRangeInvalid);
    assert_eq!(read_into(ptr::null(), 0, 1, &mut small), SbStatus::InvalidArgument);
    free(disc);

    // SAFETY: sb_status_name returns a static NUL-terminated string.
    let name = unsafe { CStr::from_ptr(sb_status_name(SbStatus::SectorRangeInvalid as i32)) };
    assert_eq!(name.to_str().unwrap(), "sector-range-invalid");
    assert_eq!(SbStatus::SectorRangeInvalid as i32, -18);

    // Logged like other reads.
    let lines = Lines(Mutex::new(Vec::new()));
    let u = ptr::from_ref(&lines).cast_mut().cast::<c_void>();
    let stored = reformat(&plain, 2064);
    let mut disc: *mut SbDisc = ptr::null_mut();
    // SAFETY: `stored` is valid for its length; `cb_log` may be called with `u` while
    // `lines` lives, which outlives the disc; `&mut disc` is a valid out-pointer.
    let s = unsafe {
        sb_disc_open_memory_with_log(stored.as_ptr(), stored.len(), Some(cb_log), u, SB_LOG_DEBUG, &mut disc)
    };
    assert_eq!(s, SbStatus::Ok);
    lines.0.lock().unwrap().clear();
    read(disc, 100, 3).unwrap();
    assert_eq!(read(disc, 546, 2), Err(SbStatus::SectorRangeInvalid));
    free(disc);
    let lines = lines.0.into_inner().unwrap();
    assert_eq!(
        lines,
        [
            (SB_LOG_DEBUG, "read 3 sectors at LSN 100: bytes 206400..212592".to_owned()),
            (SB_LOG_DEBUG, "read of 2 sectors at LSN 546: past the last sector (547)".to_owned()),
        ]
    );
}
