# scarletbook-pcm

DSD64 to 24-bit PCM conversion for Super Audio CD audio, at 88.2, 176.4 and 352.8 kHz,
deterministic per frame: converting frame `f` from frames `f - 1`, `f` and `f + 1` gives
exactly the bytes of frame `f` in a conversion of the whole area. It is part of
[scarletbook-rs](https://github.com/aclater/scarletbook-rs) and depends on no other crate of
it; it takes frames as bytes (4704 bytes per channel, channels interleaved byte by byte,
earliest sample in the MSB).

```rust
use scarletbook_pcm::{Converter, Cutoff, FrameRun, PcmConfig, PcmRate};

let mut conv = Converter::new(PcmConfig::new(PcmRate::Hz176400, Cutoff::Standard, 2))?;
let mut pcm = vec![0i32; conv.samples_per_frame() * 2];
conv.convert_frame(prev, frame, next, &mut pcm)?;     // prev/next: None at the area's edges
// or a whole run: conv.convert_run(FrameRun { before, frames, after }, &mut pcm)?
```

`Converter::convert` is the general form: it takes a run's frames in one buffer or one
buffer per frame (`&[&[u8]]`, read in place), writes `i32` samples or 3-byte little-endian
WAV/FLAC samples directly, and can report the clipped samples of each frame of the run
(the same count whether the frame is converted alone or in a run).

One `Converter` per worker thread. A `Gain` from -120 to +24 dB can be set; samples
that exceed the 24-bit range are clamped and counted per channel. 100 % modulation is full
scale, so a sustained all-ones input reports clips; Super Audio CD content stays well below
it, and clips mean the gain is too high. There is no dither.

## Filters

| Cutoff | Passband | Stopband from | Rates |
|---|---|---|---|
| `Standard` | 40 kHz | 48 kHz | 88.2, 176.4, 352.8 kHz |
| `Low` | 30 kHz | 36 kHz | 88.2, 176.4, 352.8 kHz |
| `Wide` | 80 kHz | 88.2 kHz at 176.4 kHz, 96 kHz at 352.8 kHz | 176.4, 352.8 kHz |

Multistage decimation with linear-phase (symmetric, odd-length) FIR stages, all designed
by the Parks-McClellan method in `src/design.rs`, which generates the committed
`src/tables.rs` (a test regenerates it and compares):

- S1, 2822.4 to 352.8 kHz: 63 taps (95 for `Wide`), evaluated by table lookup on the
  input bytes;
- a 31-tap halfband to 176.4 kHz (`Standard`, `Low`);
- the sharp filter at 176.4 kHz: 121 taps for `Standard`, 161 for `Low`;
- for `Wide`, a filter at 352.8 kHz: 121 taps (80/96 kHz) for 352.8 kHz output, and 233
  taps (80/88.2 kHz, decimating by 2) for 176.4 kHz output;
- 88.2 kHz takes every other 176.4 kHz sample; 352.8 kHz output of `Standard` and `Low`
  interpolates the 176.4 kHz output with the halfband (its even samples are the 176.4 kHz
  samples).

`Standard` and `Low` therefore have one response at every rate. `Wide` at 176.4 kHz stops
from that rate's Nyquist frequency, so that nothing folds back below it. Its transition, 80
to 88.2 kHz, must be made at 352.8 kHz: a filter at 176.4 kHz, or one in `z^2` at 352.8 kHz,
has a response symmetric about 88.2 kHz. Output sample `n` is centred on DSD sample
`d x n` of the area (`d` = 32, 16, 8): no delay to compensate. Only `context_bytes()`
bytes per channel of each neighbour are read: 66 to 195 bytes, depending on the rate and
cutoff.

**Silence.** The idle pattern of Super Audio CD (every byte 0x69, sacd-spec specification
0) converts to samples of exactly 0 at every rate, cutoff and gain, and so do the edges of an
area. The other idle pattern, 0x55, gives exact zeros within a run of 0x55; a change
between the two patterns (0x55 next to an area's edge or next to 0x69) is not silent,
since they are different signals. As +-1 samples, 0x69 is `-1 +1 +1 -1 +1 -1 -1 +1` repeated: each half
of a byte is the negation of the other (`x[n-4] = -x[n]`), so its mean is 0 and its
spectrum has lines only at the odd multiples of 352.8 kHz. 0x55 alternates
(`x[n-1] = -x[n]`). S1's integer taps contain the factor `(1 + z^-1)^2 (1 + z^-4)`, which
adds samples that cancel. Every S1 output over idle input is therefore exactly 0, at any bit
phase and whatever the other taps are, and the later stages keep it 0. The rest of S1 is
designed by the Remez exchange with the factor moved into the desired response and the
weight. A neighbour of `None`, the edge of an area, is the idle pattern: it gives exactly
the output of a neighbour frame of 0x69 bytes, and an area that starts or ends in silence
has no transient at its edges.

All arithmetic is integer arithmetic with round-half-to-even between stages (on a grid 8
times finer than 24 bits), carried exactly in `f64`. The bound is checked over the tables
by a test. The output is the same on every CPU, at every vector width, and however the
input is split into calls.

## Measured response

From the integer tables (`cargo test -p scarletbook-pcm --release --lib -- --ignored
--nocapture report`). The stopband is the smallest attenuation from the stopband edge to
1.4112 MHz (from 36 kHz for `Low`, and from 88.2 kHz for `Wide` at 176.4 kHz), which
covers every band that aliases into the output; for 352.8 kHz output of
`Standard` and `Low` the images of the interpolation are listed separately. Linear phase is
exact (symmetric taps).

| Rate | Cutoff | Ripple to 20 kHz | Ripple to passband edge | -0.1 dB | -3 dB | Stopband (min) |
|---|---|---|---|---|---|---|
| 88200 | Standard | ±0.0039 dB | ±0.0039 dB | 40.7 kHz | 42.7 kHz | 110.9 dB |
| 88200 | Low | ±0.0037 dB | ±0.0037 dB | 30.5 kHz | 32.0 kHz | 110.2 dB |
| 176400 | Standard | ±0.0039 dB | ±0.0039 dB | 40.7 kHz | 42.7 kHz | 110.9 dB |
| 176400 | Low | ±0.0037 dB | ±0.0037 dB | 30.5 kHz | 32.0 kHz | 110.2 dB |
| 176400 | Wide | ±0.0039 dB | ±0.0039 dB | 80.7 kHz | 82.7 kHz | 109.7 dB |
| 352800 | Standard | ±0.0039 dB | ±0.0039 dB | 40.7 kHz | 42.7 kHz | 110.9 dB (images 119.3 dB) |
| 352800 | Low | ±0.0037 dB | ±0.0037 dB | 30.5 kHz | 32.0 kHz | 110.2 dB (images 119.3 dB) |
| 352800 | Wide | ±0.0040 dB | ±0.0042 dB | 81.4 kHz | 85.3 kHz | 111.1 dB |

**The converter against its ideal filters.** The same filters in unrounded `f64`, by
direct convolution at 2.8224 MHz, on 3 frames of 2 channels. Each figure is the largest
difference in 24-bit LSB and the share of samples that differ at all. Rounding to 24 bits
alone would give at most 0.5 LSB.

| Rate | Cutoff | Largest difference | Samples differing |
|---|---|---|---|
| 88200 | Standard | 0.616 LSB | 6.79 % |
| 88200 | Low | 0.613 LSB | 6.77 % |
| 176400 | Standard | 0.616 LSB | 6.45 % |
| 176400 | Low | 0.613 LSB | 6.38 % |
| 176400 | Wide | 0.562 LSB | 6.00 % |
| 352800 | Standard | 0.679 LSB | 6.52 % |
| 352800 | Low | 0.661 LSB | 6.33 % |
| 352800 | Wide | 0.562 LSB | 6.15 % |

**Synthetic DSD.** A fifth-order sigma-delta modulator produces the input. Its noise
transfer function has zeros at DC and maximally flat poles, with a largest gain of 1.5. The
tests run with `SCARLETBOOK_PCM_REPORT=1 cargo test -p scarletbook-pcm --release --lib --
--nocapture quality`.

- **What each column measures.** The level error is the fitted level against the
  analytic response. The alignment is the fitted phase of output `n` against the input sine
  at DSD sample `d x n`, in output samples. THD+N covers the whole passband.
- **The modulator limits two of the figures.** Its shaped noise rises steeply above
  20 kHz, which sets the THD+N figures. Its signal transfer drifts where its noise transfer
  is large, because a 1-bit quantiser's gain depends on the signal. That drift accounts for
  the level and phase figures near 40 and 80 kHz.
- **What the tests check.** They check level and alignment up to 20 kHz. The analytic and
  model results above cover the converter at every frequency.

| Rate | Cutoff | Sine | Level error | Alignment | THD+N |
|---|---|---|---|---|---|
| 176400 | Standard | 1.0 kHz, -6 dBFS | +0.0000 dB | +0.00000 samples | -66.6 dB |
| 176400 | Standard | 1.0 kHz, -20 dBFS | +0.0000 dB | +0.00000 samples | -53.2 dB |
| 176400 | Standard | 10.0 kHz, -6 dBFS | +0.0000 dB | +0.00001 samples | -66.3 dB |
| 176400 | Standard | 20.0 kHz, -6 dBFS | +0.0004 dB | +0.00011 samples | -66.5 dB |
| 176400 | Standard | 39.0 kHz, -6 dBFS | +0.0200 dB | +0.00074 samples | -66.7 dB |
| 176400 | Low | 20.0 kHz, -6 dBFS | +0.0004 dB | +0.00011 samples | -80.0 dB |
| 176400 | Low | 29.0 kHz, -6 dBFS | +0.0038 dB | +0.00037 samples | -80.2 dB |
| 176400 | Wide | 20.0 kHz, -6 dBFS | +0.0004 dB | +0.00011 samples | -34.5 dB |
| 176400 | Wide | 79.0 kHz, -6 dBFS | +0.4256 dB | -0.02510 samples | -35.4 dB |
| 88200 | Standard | 20.0 kHz, -6 dBFS | +0.0004 dB | +0.00005 samples | -66.5 dB |
| 352800 | Standard | 20.0 kHz, -6 dBFS | +0.0004 dB | +0.00021 samples | -66.5 dB |

Other tests on the same input:

- **Stepped sweep.** From 20 Hz to the passband edge, the largest level error up to 20 kHz
  is 0.0001 to 0.0003 dB.
- **DC.** At ±0.5 and 0.25, the level is within 10^-4 and the gain is exact.
- **Step.** A step settles to its level, and its ring mirrors about the step instant. The
  overshoot is 5.1 % to 9.1 %, the Gibbs ringing of a linear-phase brick-wall filter.
- **Impulse.** A single flipped bit gives an output that is exactly symmetric about its
  sample.

## Throughput

`cargo run --release -p scarletbook-pcm --example throughput`: single-threaded, frame by
frame with neighbours, on a generic x86-64 build: the filters run with AVX2 and FMA, chosen at
run time, on CPUs that have them, and SSE2 otherwise (the output is the same). Measured
on an AMD Ryzen AI MAX+ 395 (Zen 5), shared with other work, pinned to one core, the best
of two runs; the figures are indicative. The requirements are at most 10 ms per second of
stereo audio at 176.4 kHz with `Standard`, and at least 100 times real time per stereo stream
for every filter at 176.4 kHz.

| Rate | Cutoff | Channels | ms CPU per s of audio | × real time |
|---|---|---|---|---|
| 88200 | Standard | 1 | 1.23 | 813 |
| 88200 | Standard | 2 | 2.63 | 380 |
| 88200 | Standard | 6 | 7.91 | 126 |
| 88200 | Low | 1 | 1.37 | 730 |
| 88200 | Low | 2 | 2.92 | 342 |
| 88200 | Low | 6 | 8.72 | 115 |
| 176400 | Standard | 1 | 1.64 | 610 |
| **176400** | **Standard** | **2** | **3.43** | **292** |
| 176400 | Standard | 6 | 10.35 | 97 |
| 176400 | Low | 1 | 1.91 | 524 |
| 176400 | Low | 2 | 3.98 | 251 |
| 176400 | Low | 6 | 11.88 | 84 |
| 176400 | Wide | 1 | 2.36 | 424 |
| 176400 | Wide | 2 | 4.88 | 205 |
| 176400 | Wide | 6 | 14.53 | 69 |
| 352800 | Standard | 1 | 2.09 | 478 |
| 352800 | Standard | 2 | 4.34 | 230 |
| 352800 | Standard | 6 | 13.02 | 77 |
| 352800 | Low | 1 | 2.36 | 424 |
| 352800 | Low | 2 | 4.87 | 205 |
| 352800 | Low | 6 | 14.60 | 68 |
| 352800 | Wide | 1 | 2.58 | 388 |
| 352800 | Wide | 2 | 5.30 | 189 |
| 352800 | Wide | 6 | 15.90 | 63 |

`cargo bench -p scarletbook-pcm --bench convert` measures the same with criterion.

## Sources

General DSP literature only, recorded in the repository's CLEANROOM.md:

- Parks & McClellan 1972, and Rabiner, McClellan & Parks 1975 (Remez exchange, also with
  a fixed factor in the response, as for the first stage's idle-pattern zeros);
- Herrmann, Rabiner & Chan 1973 (length estimates);
- Crochiere & Rabiner 1981 (multistage decimation);
- Mintzer 1982, and Vaidyanathan & Nguyen 1987 (halfband filters);
- Peled & Liu 1974, and White 1989 (bit-indexed partial-sum tables);
- Goldberg 1991 (exact integer arithmetic in IEEE-754);
- Chao et al. 1990, and Schreier & Temes 2005 (the test modulator).

## Licence

MIT OR Apache-2.0, at your option.
