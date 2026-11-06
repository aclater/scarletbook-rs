#!/usr/bin/env -S python3 -I
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Evicts a file's pages from the page cache, and checks that they left.

Usage: evict.py FILE

For the cold cases of benchmarks/runlist.json (`bench open` and `bench first` run it before each
iteration). It needs no privileges, only to own the file, and Python 3.8 on Linux: fsync (dirty
pages cannot be evicted), then posix_fadvise(POSIX_FADV_DONTNEED).

The kernel may ignore that advice (tmpfs has no backing store to re-read from), and a cold figure
taken on a cached file would look like a fast cold read. So the file's pages are counted with
mincore, on a mapping that is never touched. Do not count them with RWF_NOWAIT reads: such reads of
an evicted file put about one page per read back in the cache (seen on Linux 7.1 on btrfs), so the
count disturbs what it measures. mincore reports every page as resident to a process that neither
owns the file nor may write it, so that case fails here too, rather than passing unchecked.

Exit status 1 if any page is still cached, or if the check cannot run.
"""
import ctypes
import os
import sys

PROT_READ = 1
MAP_SHARED = 1


def cached_pages(fd, size):
    """(pages of the file in the page cache, pages of the file)."""
    libc = ctypes.CDLL(None, use_errno=True)
    libc.mmap.restype = ctypes.c_void_p
    libc.mmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_long]
    libc.munmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t]
    libc.mincore.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_char_p]
    page = os.sysconf("SC_PAGE_SIZE")
    pages = (size + page - 1) // page
    if pages == 0:
        return 0, 0
    address = libc.mmap(None, size, PROT_READ, MAP_SHARED, fd, 0)
    if address in (None, ctypes.c_void_p(-1).value):
        raise OSError(ctypes.get_errno(), "mmap")
    try:
        residence = ctypes.create_string_buffer(pages)
        if libc.mincore(address, size, residence) != 0:
            raise OSError(ctypes.get_errno(), "mincore")
    finally:
        libc.munmap(address, size)
    return sum(b & 1 for b in residence.raw[:pages]), pages


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    fd = os.open(sys.argv[1], os.O_RDONLY)
    try:
        os.fsync(fd)
        size = os.fstat(fd).st_size
        os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
        cached, pages = cached_pages(fd, size)
    except (OSError, AttributeError) as e:
        sys.exit(f"evict.py: {sys.argv[1]}: cannot evict and check: {e}")
    finally:
        os.close(fd)
    if cached:
        sys.exit(f"evict.py: {sys.argv[1]}: {cached} of {pages} pages still cached "
                 "(is it on tmpfs, or not owned by this user?)")


if __name__ == "__main__":
    main()
