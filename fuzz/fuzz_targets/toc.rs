// SPDX-License-Identifier: MIT OR Apache-2.0

//! TOC parsing: the input is the image from LSN 510 on, after 510 empty sectors.
//!
//! An input that opens is also opened through the C interface, and its whole TOC read with
//! `sb_disc_toc` (#132) as the README recommends: into a 64 KiB buffer, then into one of
//! exactly the size given. Both calls must succeed with the same size and bytes; a short
//! buffer must be refused with that size.

#![no_main]

#[path = "../../crates/scarletbook/tests/harness/mod.rs"]
mod harness;

use std::ffi::c_void;
use std::ptr;

use scarletbook_ffi::{SbDisc, SbStatus, sb_disc_free, sb_disc_open_callbacks_at, sb_disc_toc};

/// Bytes before the input: the 510 empty sectors of `harness::toc`.
const PREFIX: usize = 510 * 2048;

/// Reads the input at `offset` of the prefixed image.
///
/// # Safety
///
/// `user` must point to a live `&[u8]`; `buf` must be valid for `len` bytes of writes.
unsafe extern "C" fn read_at(user: *mut c_void, offset: u64, buf: *mut u8, len: usize) -> i64 {
    // SAFETY: `user` is the `&[u8]` passed to sb_disc_open_callbacks_at, alive for the disc.
    let data: &[u8] = unsafe { *user.cast::<&[u8]>() };
    // SAFETY: `buf` is valid for `len` bytes of writes by the contract.
    let out = unsafe { std::slice::from_raw_parts_mut(buf, len) };
    let Ok(mut at) = usize::try_from(offset) else { return 0 };
    let mut n = 0;
    if at < PREFIX {
        n = len.min(PREFIX - at);
        out[..n].fill(0);
        at += n;
    }
    let src = at.checked_sub(PREFIX).and_then(|at| data.get(at..)).unwrap_or(&[]);
    let m = (len - n).min(src.len());
    out[n..n + m].copy_from_slice(&src[..m]);
    (n + m) as i64
}

/// `sb_disc_toc` into a buffer of `cap` bytes; the status, the size given and the buffer.
fn toc(disc: *const SbDisc, cap: usize) -> (SbStatus, usize, Vec<u8>) {
    let mut buf = vec![0u8; cap];
    let mut len = usize::MAX;
    // SAFETY: `disc` is a live handle; `buf` is valid for `cap` bytes; `len` for a write.
    let s = unsafe { sb_disc_toc(disc, 0, buf.as_mut_ptr(), cap, &mut len) };
    (s, len, buf)
}

fn c_toc(data: &[u8]) {
    let mut user: &[u8] = data;
    let mut disc: *mut SbDisc = ptr::null_mut();
    let size = (PREFIX + data.len()) as u64;
    // SAFETY: `read_at` may be called with `user`, which outlives the disc, from this
    // thread; `&mut disc` is a valid out-pointer.
    let s =
        unsafe { sb_disc_open_callbacks_at(Some(read_at), size, ptr::from_mut(&mut user).cast(), &mut disc) };
    if s != SbStatus::Ok {
        return;
    }
    let (s, len, big) = toc(disc, 64 * 1024);
    assert!(s == SbStatus::Ok || (s == SbStatus::BufferTooSmall && len > 64 * 1024), "{s:?} {len}");
    let (s2, len2, exact) = toc(disc, len);
    assert_eq!((s2, len2), (SbStatus::Ok, len));
    if s == SbStatus::Ok {
        assert_eq!(big[..len], exact[..], "the same bytes in both buffers");
    }
    let (s3, len3, _) = toc(disc, len - 1);
    assert_eq!((s3, len3), (SbStatus::BufferTooSmall, len));
    // SAFETY: `disc` is a live handle, not used again.
    unsafe { sb_disc_free(disc) };
}

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    harness::toc(data);
    c_toc(data);
});
