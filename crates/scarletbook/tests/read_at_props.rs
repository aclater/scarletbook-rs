// SPDX-License-Identifier: MIT OR Apache-2.0

//! Positional readers behave as a `Cursor` over the same bytes, alone and on several threads.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{Cursor, Read, Seek, SeekFrom};
use std::sync::Arc;

use proptest::prelude::*;
use scarletbook::read_at::Locked;
use scarletbook::{Disc, ReadAt, ReadAtReader};

fn config() -> ProptestConfig {
    if cfg!(miri) {
        ProptestConfig { cases: 2, failure_persistence: None, ..ProptestConfig::default() }
    } else {
        let cases = std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(256);
        ProptestConfig { cases, ..ProptestConfig::default() }
    }
}

#[derive(Clone, Debug)]
enum Op {
    Read(usize),
    ReadExact(usize),
    Seek(SeekFrom),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0usize..300).prop_map(Op::Read),
        (0usize..300).prop_map(Op::ReadExact),
        prop_oneof![
            (0u64..400).prop_map(SeekFrom::Start),
            Just(SeekFrom::Start(u64::MAX)),
            (-400i64..400).prop_map(SeekFrom::End),
            (-400i64..400).prop_map(SeekFrom::Current),
            Just(SeekFrom::Current(i64::MAX)),
            Just(SeekFrom::End(i64::MIN)),
        ]
        .prop_map(Op::Seek),
    ]
}

/// What an operation gave: bytes or a position, or an error kind.
type Outcome = Result<(Vec<u8>, u64), std::io::ErrorKind>;

fn apply(r: &mut (impl Read + Seek), op: &Op) -> Outcome {
    let res = match op {
        Op::Read(n) => {
            let mut buf = vec![0u8; *n];
            r.read(&mut buf).map(|k| {
                buf.truncate(k);
                (buf, 0)
            })
        }
        Op::ReadExact(n) => {
            let mut buf = vec![0u8; *n];
            // A failed read_exact leaves the buffer and position unspecified: compare only
            // the error, then resynchronise with an absolute seek.
            r.read_exact(&mut buf).map(|()| (buf, 0))
        }
        Op::Seek(p) => r.seek(*p).map(|at| (Vec::new(), at)),
    };
    res.map_err(|e| e.kind())
}

fn same_as_cursor(data: &[u8], ops: &[Op], mut r: impl Read + Seek) {
    let mut c = Cursor::new(data);
    for (i, op) in ops.iter().enumerate() {
        let (x, y) = (apply(&mut r, op), apply(&mut c, op));
        assert_eq!(x, y, "op {i}: {op:?}");
        if matches!(op, Op::ReadExact(_)) && x.is_err() {
            r.seek(SeekFrom::Start(0)).unwrap();
            c.seek(SeekFrom::Start(0)).unwrap();
        }
        // Positions agree after every operation.
        assert_eq!(r.stream_position().unwrap(), c.stream_position().unwrap(), "op {i}: {op:?}");
    }
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn read_at_reader_is_a_cursor(data in proptest::collection::vec(any::<u8>(), 0..300),
                                  ops in proptest::collection::vec(op(), 0..40)) {
        let shared: Arc<[u8]> = data.clone().into();
        same_as_cursor(&data, &ops, ReadAtReader::new(Arc::clone(&shared)));
        same_as_cursor(&data, &ops, ReadAtReader::new(&data[..]));
    }

    #[test]
    fn locked_is_a_cursor(data in proptest::collection::vec(any::<u8>(), 0..300),
                          ops in proptest::collection::vec(op(), 0..40)) {
        let shared = Arc::new(Locked::new(Cursor::new(data.clone())));
        // Two readers of one locked reader: the other's reads must not move this one.
        let mut other = ReadAtReader::new(Arc::clone(&shared));
        let mut sink = [0u8; 7];
        let _ = other.read(&mut sink);
        same_as_cursor(&data, &ops, ReadAtReader::new(Arc::clone(&shared)));
    }
}

/// A file of `len` known bytes in the temporary directory, removed on drop.
struct TempFile(std::path::PathBuf);

impl TempFile {
    fn new(name: &str, data: &[u8]) -> Self {
        let path = std::env::temp_dir().join(format!("scarletbook-{}-{name}", std::process::id()));
        std::fs::write(&path, data).unwrap();
        TempFile(path)
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i as u32).wrapping_mul(2_654_435_761).to_le_bytes()[3]).collect()
}

#[test]
#[cfg_attr(miri, ignore = "file I/O")]
fn file_reads_at_offsets_from_several_threads() {
    let data = pattern(1 << 20);
    let tmp = TempFile::new("read-at", &data);
    let file = Arc::new(std::fs::File::open(&tmp.0).unwrap());
    assert_eq!(file.size().unwrap(), data.len() as u64);
    std::thread::scope(|s| {
        for t in 0..8u64 {
            let file = Arc::clone(&file);
            let data = &data;
            s.spawn(move || {
                let mut r = ReadAtReader::new(file);
                let mut buf = vec![0u8; 4096];
                for k in 0..200u64 {
                    let at = (t * 7919 + k * 104_729) % (data.len() as u64 - 4096);
                    r.seek(SeekFrom::Start(at)).unwrap();
                    r.read_exact(&mut buf).unwrap();
                    assert_eq!(&buf[..], &data[at as usize..at as usize + 4096]);
                }
                // Past the end: a short read, then nothing.
                r.seek(SeekFrom::End(-10)).unwrap();
                assert_eq!(r.read(&mut buf).unwrap(), 10);
                assert_eq!(r.read(&mut buf).unwrap(), 0);
            });
        }
    });
}

#[test]
#[cfg_attr(miri, ignore = "reads a whole corpus image")]
fn a_disc_opens_through_each_source() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/images/dst-features-5ch.iso");
    let bytes = std::fs::read(&path).unwrap();
    let want = Disc::open(Cursor::new(&bytes[..])).unwrap();
    let file = Arc::new(std::fs::File::open(&path).unwrap());
    let shared: Arc<[u8]> = bytes.clone().into();
    let tocs = [
        Disc::open(ReadAtReader::new(Arc::clone(&file))).unwrap().toc().clone(),
        Disc::open(ReadAtReader::new(shared)).unwrap().toc().clone(),
        Disc::open(ReadAtReader::new(Locked::new(std::fs::File::open(&path).unwrap())))
            .unwrap()
            .toc()
            .clone(),
    ];
    for toc in tocs {
        assert_eq!(&toc, want.toc());
    }
}
