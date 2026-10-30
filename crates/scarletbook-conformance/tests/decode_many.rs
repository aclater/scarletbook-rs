// SPDX-License-Identifier: MIT OR Apache-2.0

//! `DstDecoder::decode_many` against `decode` on the DST frames of the corpus, in random
//! mixes with damaged ones: the frames use many filters, segments and ptables.

use std::fs::File;
use std::io::BufReader;

use scarletbook::dst::{DstDecoder, FL, MAX_LANES};
use scarletbook::{Disc, FrameContent};
use scarletbook_conformance::{corpus_dir, load_cases};

#[test]
fn decode_many_equals_decode_on_corpus_frames() {
    let dir = corpus_dir();
    let mut pools: Vec<Vec<Vec<u8>>> = vec![Vec::new(); 7];
    for case in load_cases(&dir).unwrap() {
        let id = case["id"].as_str().unwrap();
        let Ok(file) = File::open(dir.join("images").join(format!("{id}.iso"))) else { continue };
        let Ok(mut disc) = Disc::open(BufReader::new(file)) else { continue };
        let areas: Vec<_> = disc.toc().areas.iter().map(|a| (a.slot, a.tracks.len() as u32)).collect();
        for (slot, tracks) in areas {
            for n in 1..=tracks {
                let Ok(frames) = disc.track_frames(slot, n) else { continue };
                let channels = usize::from(frames.channels());
                // A few frames from each track, enough to reach every shape.
                for frame in frames.take(6).flatten() {
                    if let FrameContent::Dst(bytes) = frame.content {
                        if pools[channels].len() < 120 {
                            pools[channels].push(bytes);
                        }
                    }
                }
            }
        }
    }
    let mut seed = 0x1234_5678_9ABC_DEF1u64;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut compared = 0;
    for channels in 1..=6u8 {
        let pool = &pools[usize::from(channels)];
        if pool.is_empty() {
            continue;
        }
        let mut singles = DstDecoder::new();
        let mut many = DstDecoder::new();
        for _ in 0..24 {
            let count = 1 + (rnd() as usize) % (2 * MAX_LANES);
            let frames: Vec<Vec<u8>> = (0..count)
                .map(|_| {
                    let mut f = pool[(rnd() as usize) % pool.len()].clone();
                    match rnd() % 8 {
                        0 => f.truncate((rnd() as usize) % f.len()),
                        1 => {
                            let i = (rnd() as usize) % f.len();
                            f[i] ^= 1 << (rnd() % 8);
                        }
                        _ => {}
                    }
                    f
                })
                .collect();
            let refs: Vec<&[u8]> = frames.iter().map(Vec::as_slice).collect();
            let mut bufs = vec![vec![0u8; FL * usize::from(channels)]; count];
            let mut outs: Vec<&mut [u8]> = bufs.iter_mut().map(Vec::as_mut_slice).collect();
            let results = many.decode_many(&refs, channels, &mut outs);
            for (i, f) in refs.iter().enumerate() {
                let mut want = vec![0u8; FL * usize::from(channels)];
                let r = singles.decode(f, channels, &mut want);
                assert_eq!(results[i], r, "{channels} channels, frame {i} of {count}");
                if r.is_ok() {
                    assert!(bufs[i] == want, "{channels} channels, frame {i} of {count}: output differs");
                }
                compared += 1;
            }
        }
    }
    assert!(compared > 300, "too few frames compared: {compared}");
    assert!(
        pools.iter().filter(|p| !p.is_empty()).count() >= 2,
        "corpus DST frames of too few channel counts"
    );
}
