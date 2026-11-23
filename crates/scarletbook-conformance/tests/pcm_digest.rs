// SPDX-License-Identifier: MIT OR Apache-2.0

//! The PCM conversion of the corpus's audio, pinned (issue #216): every area of every
//! corpus image that reads (each distinct one once, its first frames), converted at every
//! rate and cutoff as a filesystem converts it (calls of a few frames each, separate frame
//! buffers, both neighbours, 3-byte samples and per-frame clip counts; at +12 dB, which
//! clips) and as one whole run of `i32` samples (at 0 dB); then the same for synthetic
//! areas of 1 to 6 channels. The digest of all of it is the output of `scarletbook-pcm`
//! 0.5.3, so a change to the converter's code that changes any sample or count fails here.

use std::fs::File;
use std::io::BufReader;

use scarletbook::{Disc, Slot};
use scarletbook_conformance::{corpus_dir, load_cases};
use scarletbook_pcm::{
    Converter, Cutoff, FRAME_BYTES_PER_CHANNEL, Frames, Gain, PcmConfig, PcmOut, PcmRate, Run,
};

/// FNV-1a, 64 bits (Fowler, Noll and Vo), over the bytes fed to it.
struct Fnv(u64);

impl Fnv {
    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 = (self.0 ^ u64::from(x)).wrapping_mul(0x100_0000_01b3);
        }
    }

    fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }
}

/// The digest of the conversions of the corpus at 0.5.3.
const DIGEST: u64 = 0xec6e_a5f2_49df_0f9f;

/// Frames converted per area, from its start.
const MAX_FRAMES: usize = 12;

/// Frames per call, in turn: sizes around one read of a filesystem.
const CALLS: [usize; 4] = [1, 3, 8, 5];

#[test]
fn corpus_conversion_is_unchanged() {
    let dir = corpus_dir();
    let cases = load_cases(&dir).unwrap_or_else(|e| panic!("manifest: {e}"));
    let mut h = Fnv(0xcbf2_9ce4_8422_2325);
    // Many images share their audio: each distinct area once.
    let mut seen = std::collections::HashSet::new();
    let (mut areas, mut clipped) = (0, 0);
    for case in &cases {
        let id = case["id"].as_str().unwrap_or("?");
        let file = File::open(dir.join("images").join(format!("{id}.iso"))).unwrap();
        let Ok(mut disc) = Disc::open(BufReader::new(file)) else { continue };
        for slot in Slot::ALL {
            let Some(area) = disc.toc().area(slot) else { continue };
            let (playtime, channels) = (area.playtime_frames(), area.channel_count);
            if !(1..=6).contains(&channels) || disc.check_range(slot, 0, playtime).is_err() {
                continue;
            }
            let Ok(audio) = disc.range_audio(slot, 0, playtime) else { continue };
            let frames: Vec<Vec<u8>> = audio.map_while(Result::ok).take(MAX_FRAMES).map(|f| f.data).collect();
            if frames.is_empty() || !seen.insert(frames.clone()) {
                continue;
            }
            areas += 1;
            h.u64(frames.len() as u64);
            clipped += convert_area(&mut h, channels, &frames);
        }
    }
    assert!(areas >= 10, "only {areas} areas read");
    for channels in 1..=6u8 {
        clipped += convert_area(&mut h, channels, &synthetic(channels, 7));
    }
    assert!(clipped > 0, "nothing clipped");
    assert_eq!(h.0, DIGEST, "{:#018x}", h.0);
}

/// `frames` frames of pseudo-random bytes with stretches of 0xFF and 0x00, which clip.
fn synthetic(channels: u8, frames: usize) -> Vec<Vec<u8>> {
    let mut s = 0x2545_F491_4F6C_DD1Du64 ^ u64::from(channels);
    let mut constant = None;
    let frame = usize::from(channels) * FRAME_BYTES_PER_CHANNEL;
    (0..frames)
        .map(|_| {
            (0..frame)
                .map(|i| {
                    s ^= s << 13;
                    s ^= s >> 7;
                    s ^= s << 17;
                    if i % 4096 == 0 {
                        constant = (s % 3 == 0).then_some(if s % 2 == 0 { 0xFF } else { 0x00 });
                    }
                    constant.unwrap_or(s as u8)
                })
                .collect()
        })
        .collect()
}

/// Converts `frames` in both ways at every rate and cutoff, into `h`; returns the samples
/// clipped in the calls.
fn convert_area(h: &mut Fnv, channels: u8, frames: &[Vec<u8>]) -> u64 {
    let mut total = 0;
    let list: Vec<&[u8]> = frames.iter().map(Vec::as_slice).collect();
    let whole = frames.concat();
    for rate in PcmRate::ALL {
        for cutoff in Cutoff::ALL.into_iter().filter(|c| c.supports(rate)) {
            let config = PcmConfig::new(rate, cutoff, channels);
            // A filesystem's calls, at +12 dB.
            let mut conv = Converter::new(config.with_gain(Gain::from_db(12.0).unwrap())).unwrap();
            let per = conv.samples_per_frame() * usize::from(channels);
            let mut wav = vec![0u8; 3 * per * list.len()];
            let mut clipped = vec![0u64; list.len()];
            let (mut f, mut i) = (0, 0);
            while f < list.len() {
                let k = CALLS[i % CALLS.len()].min(list.len() - f);
                let run = Run::new(Frames::Separate(&list[f..f + k]))
                    .with_before(f.checked_sub(1).map(|p| list[p]))
                    .with_after(list.get(f + k).copied());
                let out = PcmOut::S24le(&mut wav[3 * per * f..3 * per * (f + k)]);
                let stats = conv.convert(run, out, Some(&mut clipped[f..f + k])).unwrap();
                h.u64(stats.clipped_total());
                f += k;
                i += 1;
            }
            h.bytes(&wav);
            clipped.iter().for_each(|&c| h.u64(c));
            total += clipped.iter().sum::<u64>();
            // One run of `i32` samples, at 0 dB.
            let mut conv = Converter::new(config).unwrap();
            let mut pcm = vec![0i32; per * list.len()];
            let stats = conv.convert(Run::new(&whole[..]), PcmOut::I32(&mut pcm), None).unwrap();
            stats.clipped.iter().for_each(|&c| h.u64(c));
            pcm.iter().for_each(|s| h.bytes(&s.to_le_bytes()));
        }
    }
    total
}
