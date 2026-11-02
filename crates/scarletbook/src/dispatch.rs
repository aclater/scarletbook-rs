// SPDX-License-Identifier: MIT OR Apache-2.0

//! The instruction set of the DST decoder and of the output path, chosen at run time.
//!
//! On x86-64 with AVX2, BMI1, BMI2, LZCNT and MOVBE, two pieces of safe code run as builds
//! with those features enabled (`#[target_feature]`), through one wrapper,
//! `avx2::run`:
//! - a group of DST frames ([`decode_group`]): LZCNT and the BMI shifts shorten the
//!   arithmetic decoder's serial chain, MOVBE loads its big-endian code bits, and AVX2
//!   (with the 3-operand AVX encoding) sums the prediction rows (#146);
//! - [`crate::output::Layout::fill`] and [`crate::output::Layout::read_at`]
//!   ([`output_path`]): AVX2 separates the channels of DSF rows (#202).
//!
//! Both are integer-only, so the output is the same to the bit. Everywhere else the code
//! runs as compiled.
//!
//! This is the only module of the crate with `unsafe` (CONTRIBUTING): calling a
//! `#[target_feature]` function is `unsafe` unless the caller has the same features (Rust
//! 1.86; before it, such functions must be `unsafe fn`). Each call is made only with an
//! [`Isa`] that [`Isa::detect`] built after `is_x86_feature_detected!` found every feature.
//! The wrapper holds no other unsafe operation: no intrinsics, no raw pointers. The
//! exception is to be revisited under #199, for a safe alternative such as struct target
//! features (RFC 3525) or per-CPU builds.

#![allow(unsafe_code)]

use crate::dst::{DstDecoder, DstError};

/// The instruction set the decoder and the output path use; only [`Isa::detect`] and
/// [`Isa::generic`] make one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Isa(Level);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Level {
    Generic,
    #[cfg(target_arch = "x86_64")]
    Avx2Bmi,
}

impl Isa {
    /// The best instruction set this CPU has.
    pub(crate) fn detect() -> Isa {
        #[cfg(target_arch = "x86_64")]
        if std::arch::is_x86_feature_detected!("avx2")
            && std::arch::is_x86_feature_detected!("bmi1")
            && std::arch::is_x86_feature_detected!("bmi2")
            && std::arch::is_x86_feature_detected!("lzcnt")
            && std::arch::is_x86_feature_detected!("movbe")
        {
            return Isa(Level::Avx2Bmi);
        }
        Isa::generic()
    }

    /// The code as compiled, without run-time features.
    pub(crate) fn generic() -> Isa {
        Isa(Level::Generic)
    }
}

/// Work run with the features of an [`Isa`]. `run` must be `#[inline(always)]`, and so
/// must everything it calls on the hot path, to be compiled with those features: a
/// function that isn't inlined into `avx2::run` is built without them. (A closure is not
/// enough: its body was outlined from the `#[target_feature]` function.)
pub(crate) trait Work {
    type Output;
    fn run(self) -> Self::Output;
}

/// Runs `work` with the features of `isa`: through `avx2::run` for `Level::Avx2Bmi`.
#[inline(always)]
fn run<W: Work>(isa: Isa, work: W) -> W::Output {
    match isa.0 {
        // SAFETY: `Level::Avx2Bmi` is made only by `Isa::detect`, after
        // `is_x86_feature_detected!` found AVX2, BMI1, BMI2, LZCNT and MOVBE.
        #[cfg(target_arch = "x86_64")]
        Level::Avx2Bmi => unsafe { avx2::run(work) },
        Level::Generic => work.run(),
    }
}

/// [`DstDecoder::group`].
pub(crate) fn decode_group(
    isa: Isa,
    dec: &mut DstDecoder,
    frames: &[&[u8]],
    channels: u8,
    outs: &mut [&mut [u8]],
    results: &mut [Result<(), DstError>],
) {
    if isa == Isa::generic() {
        dec.group::<false>(frames, channels, outs, results)
    } else {
        run(isa, Group { dec, frames, channels, outs, results })
    }
}

/// A group of DST frames decoded with the fast loops ([`DstDecoder::group`] `<true>`).
struct Group<'a, 'b, 'c> {
    dec: &'a mut DstDecoder,
    frames: &'a [&'b [u8]],
    channels: u8,
    outs: &'a mut [&'c mut [u8]],
    results: &'a mut [Result<(), DstError>],
}

impl Work for Group<'_, '_, '_> {
    type Output = ();
    #[inline(always)]
    fn run(self) {
        self.dec.group::<true>(self.frames, self.channels, self.outs, self.results)
    }
}

/// Runs `work`, the whole of [`crate::output::Layout::fill`] or
/// [`crate::output::Layout::read_at`], with the features of `isa`. What `work` inlines is
/// built with them: the filling of DSF and DSDIFF byte ranges, channel separation
/// included. Reading and demultiplexing the area are behind the `dyn`
/// [`crate::output::PositionSource`] call and run as compiled; DST frames, decoded in
/// groups (on a rayon pool with `parallel`), keep [`decode_group`]'s dispatch.
#[cfg(feature = "output")]
#[inline(always)]
pub(crate) fn output_path<W: Work>(isa: Isa, work: W) -> W::Output {
    run(isa, work)
}

/// The one `#[target_feature]` wrapper; callers must have checked that the CPU has every
/// feature.
#[cfg(target_arch = "x86_64")]
mod avx2 {
    /// `work` and everything it inlines, built with AVX2, BMI1, BMI2, LZCNT and MOVBE.
    #[target_feature(enable = "avx2,bmi1,bmi2,lzcnt,movbe")]
    pub(super) unsafe fn run<W: super::Work>(work: W) -> W::Output {
        work.run()
    }
}

#[cfg(test)]
#[path = "../tests/common/dst_enc.rs"]
mod dst_enc;

#[cfg(all(test, feature = "output"))]
#[path = "../tests/common/mod.rs"]
mod common;

/// The output path ([`output_path`]) with the detected instruction set gives the bytes of
/// the generic one.
#[cfg(all(test, feature = "output"))]
mod output_tests {
    use std::borrow::Cow;
    use std::io::{Read, Seek};

    use super::Isa;
    use super::common::{PFrame, Spec, TrackSpec, pack};
    use crate::audio::FRAME_BYTES_PER_CHANNEL;
    use crate::output::{DsfBitsPerSample, Format, Layout, OutputOptions, PositionSource, Tags};
    use crate::{Disc, Error, Slot};

    /// The positions `[first, ..)` held in memory.
    struct Held {
        first: u32,
        content: Vec<Vec<u8>>,
    }

    impl PositionSource for Held {
        fn content(&mut self, t: u32) -> Result<Cow<'_, [u8]>, Error> {
            let i = t.checked_sub(self.first).ok_or(Error::InvalidArgument("before the range"))?;
            let c = self.content.get(i as usize).ok_or(Error::InvalidArgument("past the range"))?;
            Ok(Cow::Borrowed(c))
        }
    }

    /// One read of the whole file and reads of `chunk` bytes, or of a quarter of the file
    /// if that is more (an odd count, so that they start and end inside DSF rows), through
    /// `read_at` and through `fill`, with `isa`.
    fn reads<R: Read + Seek>(isa: Isa, disc: &mut Disc<R>, layout: &Layout, chunk: usize) -> Vec<Vec<u8>> {
        let size = layout.size() as usize;
        // Each `read_at` reads the area from its start again: few of them under Miri.
        let chunk = chunk.max((size / if cfg!(miri) { 2 } else { 4 }) | 1);
        let positions = layout.positions_for(0..layout.size());
        let content = if positions.is_empty() {
            Vec::new()
        } else {
            let frames = disc.range_audio(layout.slot(), positions.start, positions.end).unwrap();
            frames.map(|f| f.unwrap().data).collect()
        };
        let mut source = Held { first: positions.start, content };
        let mut whole = vec![0xAA; size];
        assert_eq!(layout.read_at_with(isa, disc, 0, &mut whole).unwrap(), size);
        let mut filled = vec![0x55; size];
        assert_eq!(layout.fill_with(isa, 0, &mut filled, &mut source).unwrap(), size);
        let (mut by_read, mut by_fill) = (Vec::new(), Vec::new());
        for at in (0..size).step_by(chunk) {
            let mut part = vec![0; chunk.min(size - at)];
            layout.read_at_with(isa, disc, at as u64, &mut part).unwrap();
            by_read.extend_from_slice(&part);
            layout.fill_with(isa, at as u64, &mut part, &mut source).unwrap();
            by_fill.extend_from_slice(&part);
        }
        vec![whole, filled, by_read, by_fill]
    }

    /// The layouts of `[first, end)` of the area from `slot`: DSF with both bit orders and
    /// DSDIFF; and the DSF files of a chain of the range split in two, when it can be.
    /// Under Miri: DSF with bits reversed (the default), DSDIFF, and the chain's second
    /// file, which starts inside a position and a DSF row (8.2.7).
    fn layouts<R: Read + Seek>(disc: &Disc<R>, slot: Slot, first: u32, end: u32) -> Vec<Layout> {
        let area = disc.toc().area(slot).unwrap();
        let channel_type = [1, 2, 3, 4, 6, 7][usize::from(area.channel_count) - 1];
        let options = OutputOptions::default()
            .with_tags(Tags::None)
            .with_dsf_unspecified_channel_type(Some(channel_type));
        let mut out = Vec::new();
        let bits: &[_] = if cfg!(miri) {
            &[DsfBitsPerSample::One]
        } else {
            &[DsfBitsPerSample::One, DsfBitsPerSample::Eight]
        };
        for &bits in bits {
            let o = options.clone().with_dsf_bits_per_sample(bits);
            out.extend(disc.output_layout(slot, first, end, Format::Dsf, &o));
        }
        out.extend(disc.output_layout(slot, first, end, Format::DsdiffDsd, &options));
        if end - first >= 2 {
            let mid = first + (end - first) / 2;
            let files = disc.plan_dsf_chain(slot, &[first, mid, end]).unwrap().into_iter().flatten();
            for file in files.skip(usize::from(cfg!(miri))) {
                let o = options.clone().with_dsf_chain(Some(file.chain));
                out.extend(disc.output_layout(slot, file.range.0, file.range.1, Format::Dsf, &o));
            }
        }
        out
    }

    /// Every layout of every range in `ranges` reads the same with [`Isa::detect`] and
    /// [`Isa::generic`]; returns how many layouts were compared.
    fn compare<R: Read + Seek>(disc: &mut Disc<R>, slot: Slot, ranges: &[(u32, u32)], chunk: usize) -> usize {
        let mut n = 0;
        for &(first, end) in ranges {
            if disc.check_range(slot, first, end).is_err() {
                continue;
            }
            for layout in layouts(disc, slot, first, end) {
                let detected = reads(Isa::detect(), disc, &layout, chunk);
                let generic = reads(Isa::generic(), disc, &layout, chunk);
                for (i, (d, g)) in detected.iter().zip(&generic).enumerate() {
                    assert!(
                        d == g,
                        "{slot} [{first}, {end}) {:?}, read {i} differs ({:?})",
                        layout.format(),
                        Isa::detect()
                    );
                    assert!(
                        *d == generic[0],
                        "{slot} [{first}, {end}) {:?}: read {i} differs from the whole read",
                        layout.format()
                    );
                }
                n += 1;
            }
        }
        n
    }

    /// A synthetic area of `channels` channels and `n` plain positions, some missing, in
    /// two tracks.
    fn synthetic(channels: u8, n: u32) -> Disc<std::io::Cursor<Vec<u8>>> {
        let frames: Vec<PFrame> = (0..n)
            .filter(|t| t % 4 != 2)
            .map(|number| PFrame {
                number,
                dst: false,
                data: (0..usize::from(channels) * FRAME_BYTES_PER_CHANNEL)
                    .map(|i| (i as u32 * 7 + number * 13 + i as u32 / 251) as u8)
                    .collect(),
            })
            .collect();
        let tracks =
            vec![TrackSpec { start: 0, duration: n / 2 }, TrackSpec { start: n / 2, duration: n - n / 2 }];
        let spec = Spec { channels, playtime: n, tracks, audio: pack(&frames) };
        Disc::open(std::io::Cursor::new(spec.build())).unwrap()
    }

    /// Synthetic areas of 1 to 6 channels and the areas of the corpus (plain and DST, 1
    /// to 6 channels): the first track, with and without pause, the play range, and chains
    /// (8.2.7), as DSF and DSDIFF, through `read_at` and `fill`, whole and in reads that
    /// start and end inside DSF rows. (On a CPU without the features, both are generic.)
    #[test]
    #[cfg_attr(miri, ignore = "the corpus is too large for Miri; see detected_output_matches_generic_simple")]
    fn detected_output_matches_generic() {
        let mut n = 0;
        for channels in 1..=6 {
            let mut disc = synthetic(channels, 9);
            n += compare(&mut disc, Slot::TwoChannel, &[(0, 9), (1, 4), (4, 9), (2, 3)], 5000);
        }
        let images = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/images");
        let mut paths: Vec<_> = std::fs::read_dir(images).unwrap().map(|e| e.unwrap().path()).collect();
        paths.sort();
        let mut seen = [false; 7];
        for path in paths.iter().filter(|p| p.extension().is_some_and(|e| e == "iso")) {
            let Ok(file) = std::fs::File::open(path) else { continue };
            let Ok(mut disc) = Disc::open(std::io::BufReader::new(file)) else { continue };
            for area in disc.toc().areas.clone() {
                let mut ranges = vec![(0, area.playtime_frames())];
                for t in area.tracks.iter().take(1) {
                    ranges.push((t.start_frame, t.end_frame()));
                    ranges.extend(disc.track_range_with_pause(area.slot, u32::from(t.number)));
                }
                let compared = compare(&mut disc, area.slot, &ranges, 7001);
                seen[usize::from(area.channel_count).min(6)] |= compared > 0;
                n += compared;
            }
        }
        assert!(seen[1..].iter().all(|&s| s), "channel counts compared from the corpus: {seen:?}");
        assert!(n > 200, "only {n} layouts compared");
    }

    /// The same on small synthetic areas of 1, 2, 5 and 6 channels: a range and its chain
    /// of two files. Under Miri (the `core-dispatch` slice), 2 channels and three of the
    /// layouts ([`layouts`]).
    #[test]
    fn detected_output_matches_generic_simple() {
        let channels: &[u8] = if cfg!(miri) { &[2] } else { &[1, 2, 5, 6] };
        let mut n = 0;
        for &c in channels {
            let n_frames = if cfg!(miri) { 2 } else { 3 };
            let mut disc = synthetic(c, n_frames);
            n += compare(&mut disc, Slot::TwoChannel, &[(0, n_frames)], 5000);
        }
        let per_area = if cfg!(miri) { 3 } else { 5 };
        assert!(n >= per_area * channels.len(), "only {n} layouts compared");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dst::{FL, MAX_LANES};
    use crate::{Disc, FrameContent};

    /// Up to `per_track` DST frames of every track of every corpus image, by channel count.
    fn corpus_frames(per_track: usize) -> Vec<Vec<Vec<u8>>> {
        let images = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/images");
        let mut pools = vec![Vec::new(); 7];
        let mut paths: Vec<_> = std::fs::read_dir(images).unwrap().map(|e| e.unwrap().path()).collect();
        paths.sort();
        for path in paths.iter().filter(|p| p.extension().is_some_and(|e| e == "iso")) {
            let Ok(file) = std::fs::File::open(path) else { continue };
            let Ok(mut disc) = Disc::open(std::io::BufReader::new(file)) else { continue };
            let areas: Vec<_> = disc.toc().areas.iter().map(|a| (a.slot, a.tracks.len() as u32)).collect();
            for (slot, tracks) in areas {
                for n in 1..=tracks {
                    let Ok(frames) = disc.track_frames(slot, n) else { continue };
                    let channels = usize::from(frames.channels());
                    for frame in frames.take(per_track).flatten() {
                        if let FrameContent::Dst(bytes) = frame.content {
                            pools[channels].push(bytes);
                        }
                    }
                }
            }
        }
        pools
    }

    /// Every frame of `pool` and a damaged copy of every third one, decoded in groups of 1 to
    /// [`MAX_LANES`] frames with `isa`: the results and the outputs.
    fn decode_all(isa: Isa, pool: &[Vec<u8>], channels: u8) -> Vec<(Result<(), DstError>, Vec<u8>)> {
        let mut frames: Vec<Vec<u8>> = pool.to_vec();
        for (i, f) in pool.iter().enumerate().step_by(3) {
            let mut f = f.clone();
            if i % 2 == 0 {
                f.truncate(f.len() * 2 / 3);
            } else {
                let at = (i * 7919) % f.len();
                f[at] ^= 1 << (i % 8);
            }
            frames.push(f);
        }
        let mut dec = DstDecoder::with_isa(isa);
        let mut out = Vec::new();
        let mut rest = &frames[..];
        let mut group = 1;
        while !rest.is_empty() {
            let (now, later) = rest.split_at(group.min(rest.len()));
            let refs: Vec<&[u8]> = now.iter().map(Vec::as_slice).collect();
            let mut bufs = vec![vec![0u8; FL * usize::from(channels)]; now.len()];
            let mut outs: Vec<&mut [u8]> = bufs.iter_mut().map(Vec::as_mut_slice).collect();
            let results = dec.decode_many(&refs, channels, &mut outs);
            out.extend(
                results.into_iter().zip(bufs).map(|(r, b)| (r, if r.is_ok() { b } else { Vec::new() })),
            );
            rest = later;
            group = group % MAX_LANES + 1;
        }
        out
    }

    fn assert_same(pool: &[Vec<u8>], channels: u8, what: &str) {
        let detected = decode_all(Isa::detect(), pool, channels);
        let generic = decode_all(Isa::generic(), pool, channels);
        assert_eq!(detected.len(), generic.len());
        assert!(generic.iter().any(|(r, _)| r.is_ok()), "{what}, {channels} channels: no valid frame");
        for (i, (d, g)) in detected.iter().zip(&generic).enumerate() {
            assert!(
                d == g,
                "{what}, {channels} channels, frame {i}: {:?} differs ({:?})",
                d.0,
                Isa::detect()
            );
        }
    }

    /// The decoder chosen for this CPU gives exactly the output and results of the generic
    /// decoder on the DST frames of the corpus (1 to 6 channels), on the generated stress
    /// frames of the benchmarks (2 and 6 channels), and on damaged copies of both. (On a
    /// CPU without the features, and under Miri, both are the generic decoder.)
    #[test]
    #[cfg_attr(
        miri,
        ignore = "the corpus is too large for Miri; see detected_decoder_matches_generic_simple"
    )]
    fn detected_decoder_matches_generic() {
        let pools = corpus_frames(6);
        let mut seen = 0;
        for channels in 1..=6u8 {
            let pool = &pools[usize::from(channels)];
            assert!(!pool.is_empty(), "the corpus has no {channels}-channel DST frames");
            assert_same(pool, channels, "corpus");
            seen += pool.len();
        }
        for nch in [2u8, 6] {
            let pool: Vec<Vec<u8>> =
                dst_enc::stress_set(2).into_iter().filter(|f| f.channels == nch).map(|f| f.frame).collect();
            assert!(!pool.is_empty());
            assert_same(&pool, nch, "stress");
            seen += pool.len();
        }
        assert!(seen > 100, "only {seen} frames");
    }

    /// The same on small generated frames, short enough for Miri.
    #[test]
    fn detected_decoder_matches_generic_simple() {
        let mut seed = 0x51_3C_7A_D2u64;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let coefs: Vec<i32> = (0..20).map(|i| [96, -40, 17, -5][i % 4] >> (i / 4)).collect();
        let ptable: Vec<u32> = (0..24).map(|i| 128 - 5 * i).collect();
        let channels: &[u8] = if cfg!(miri) { &[1] } else { &[1, 2, 3, 4, 5, 6] };
        for &nch in channels {
            let pool: Vec<Vec<u8>> = (0..if cfg!(miri) { 1 } else { 6 })
                .map(|k| {
                    let signal: Vec<Vec<u8>> =
                        (0..nch).map(|_| (0..FL).map(|_| rnd() as u8).collect()).collect();
                    let half: Vec<bool> = (0..nch).map(|c| (usize::from(c) + k) % 3 == 0).collect();
                    dst_enc::encode_simple(&signal, &coefs, &ptable, &half)
                })
                .collect();
            assert_same(&pool, nch, "generated");
        }
    }
}
