# scarletbook-pcm

DSD64 to 24-bit PCM conversion for Super Audio CD audio, at 88.2, 176.4 and 352.8 kHz,
deterministic per frame: converting frame `f` from frames `f - 1`, `f` and `f + 1` gives
exactly the bytes of frame `f` in a conversion of the whole area. It is part of
[scarletbook-rs](https://github.com/aclater/scarletbook-rs) and depends on no other crate of
it; it takes frames as bytes (4704 bytes per channel, channels interleaved byte by byte,
earliest sample in the MSB).

```rust
use scarletbook_pcm::{Converter, Cutoff, FrameRun, PcmConfig, PcmRate};

let mut conv = Converter::new(PcmConfig::new(PcmRate::Hz176400, Cutoff::Default, 2))?;
let mut pcm = vec![0i32; conv.samples_per_frame() * 2];
conv.convert_frame(prev, frame, next, &mut pcm)?;     // prev/next: None at the area's edges
// or a whole run: conv.convert_run(FrameRun { before, frames, after }, &mut pcm)?
```

One `Converter` per worker thread. A `Gain` from -120 to +24 dB can be set; samples
that exceed the 24-bit range are clamped and counted per channel. 100 % modulation is full
scale, so a sustained all-ones input reports clips; Super Audio CD content stays well below
it, and clips mean the gain is too high. There is no dither.

## Filters

| Cutoff | Passband | Stopband from | Rates |
|---|---|---|---|
| `Default` | 40 kHz | 48 kHz | 88.2, 176.4, 352.8 kHz |
| `Low` | 30 kHz | 38 kHz | 88.2, 176.4, 352.8 kHz |
| `Wide` | 80 kHz | 96 kHz | 176.4, 352.8 kHz |

Multistage decimation with linear-phase (symmetric, odd-length) FIR stages, all designed
by the Parks-McClellan method in `src/design.rs`, which generates the committed
`src/tables.rs` (a test regenerates it and compares):

- S1, 2822.4 to 352.8 kHz: 63 taps (95 for `Wide`), evaluated by table lookup on the
  input bytes;
- a 31-tap halfband to 176.4 kHz (`Default`, `Low`);
- the sharp filter, 121 taps, at 176.4 kHz (`Default`, `Low`) or 352.8 kHz (`Wide`);
- 88.2 kHz takes every other 176.4 kHz sample; 352.8 kHz output of `Default` and `Low`
  interpolates the 176.4 kHz output with the halfband (its even samples are the 176.4 kHz
  samples).

Each cutoff therefore has one response at every rate. Output sample `n` is centred on DSD
sample `d x n` of the area (`d` = 32, 16, 8): no delay to compensate. A neighbour of `None`
is silence (the value 0, not a bit pattern). Only `context_bytes()` bytes per channel of
each neighbour are read: 66 to 155 bytes, depending on the rate and cutoff.

All arithmetic is integer arithmetic with round-half-to-even between stages (on a grid 8
times finer than 24 bits), carried exactly in `f64`. The bound is checked over the tables
by a test. The output is the same on every CPU, at every vector width, and however the
input is split into calls.

## Measured response

From the integer tables (`cargo test -p scarletbook-pcm --release --lib -- --ignored
--nocapture report`). The stopband is the smallest attenuation from the stopband edge to
1.4112 MHz, which covers every band that aliases into the output; for 352.8 kHz output of
`Default` and `Low` the images of the interpolation are listed separately. Linear phase is
exact (symmetric taps).

| Rate | Cutoff | Ripple to 20 kHz | Ripple to passband edge | -0.1 dB | -3 dB | Stopband (min) |
|---|---|---|---|---|---|---|
| 88200 | Default | ±0.0039 dB | ±0.0039 dB | 40.7 kHz | 42.7 kHz | 110.9 dB |
| 88200 | Low | ±0.0039 dB | ±0.0039 dB | 30.7 kHz | 32.7 kHz | 110.0 dB |
| 176400 | Default | ±0.0039 dB | ±0.0039 dB | 40.7 kHz | 42.7 kHz | 110.9 dB |
| 176400 | Low | ±0.0039 dB | ±0.0039 dB | 30.7 kHz | 32.7 kHz | 110.0 dB |
| 176400 | Wide | ±0.0040 dB | ±0.0042 dB | 81.4 kHz | 85.3 kHz | 111.1 dB |
| 352800 | Default | ±0.0039 dB | ±0.0039 dB | 40.7 kHz | 42.7 kHz | 110.9 dB (images 119.3 dB) |
| 352800 | Low | ±0.0039 dB | ±0.0039 dB | 30.7 kHz | 32.7 kHz | 110.0 dB (images 119.3 dB) |
| 352800 | Wide | ±0.0040 dB | ±0.0042 dB | 81.4 kHz | 85.3 kHz | 111.1 dB |

**The converter against its ideal filters.** The same filters in unrounded `f64`, by
direct convolution at 2.8224 MHz, on 3 frames of 2 channels. Each figure is the largest
difference in 24-bit LSB and the share of samples that differ at all. Rounding to 24 bits
alone would give at most 0.5 LSB.

| Rate | Cutoff | Largest difference | Samples differing |
|---|---|---|---|
| 88200 | Default | 0.610 LSB | 6.36 % |
| 88200 | Low | 0.602 LSB | 6.42 % |
| 176400 | Default | 0.619 LSB | 6.26 % |
| 176400 | Low | 0.619 LSB | 6.28 % |
| 176400 | Wide | 0.562 LSB | 6.14 % |
| 352800 | Default | 0.656 LSB | 6.52 % |
| 352800 | Low | 0.679 LSB | 6.47 % |
| 352800 | Wide | 0.562 LSB | 6.12 % |

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
| 176400 | Default | 1.0 kHz, -6 dBFS | +0.0000 dB | +0.00000 samples | -66.6 dB |
| 176400 | Default | 1.0 kHz, -20 dBFS | +0.0000 dB | +0.00004 samples | -53.2 dB |
| 176400 | Default | 10.0 kHz, -6 dBFS | +0.0000 dB | +0.00001 samples | -66.3 dB |
| 176400 | Default | 20.0 kHz, -6 dBFS | +0.0004 dB | +0.00011 samples | -66.5 dB |
| 176400 | Default | 39.0 kHz, -6 dBFS | +0.0200 dB | +0.00074 samples | -66.7 dB |
| 176400 | Low | 20.0 kHz, -6 dBFS | +0.0004 dB | +0.00011 samples | -78.9 dB |
| 176400 | Low | 29.0 kHz, -6 dBFS | +0.0038 dB | +0.00037 samples | -79.0 dB |
| 176400 | Wide | 20.0 kHz, -6 dBFS | +0.0004 dB | +0.00011 samples | -33.0 dB |
| 176400 | Wide | 79.0 kHz, -6 dBFS | +0.4255 dB | -0.02510 samples | -33.7 dB |
| 88200 | Default | 20.0 kHz, -6 dBFS | +0.0004 dB | +0.00005 samples | -66.5 dB |
| 352800 | Default | 20.0 kHz, -6 dBFS | +0.0004 dB | +0.00021 samples | -66.5 dB |

Other tests on the same input:

- **Stepped sweep.** From 20 Hz to the passband edge, the largest level error up to 20 kHz
  is 0.0001 to 0.0003 dB.
- **DC.** At ±0.5 and 0.25, the level is within 10^-4 and the gain is exact.
- **Step.** A step settles to its level, and its ring mirrors about the step instant. The
  overshoot is 5.6 % to 9.1 %, the Gibbs ringing of a linear-phase brick-wall filter.
- **Impulse.** A single flipped bit gives an output that is exactly symmetric about its
  sample.

## Throughput

`cargo run --release -p scarletbook-pcm --example throughput`: single-threaded, frame by
frame with neighbours, on a generic x86-64 build (SSE2 only, no runtime dispatch). Measured
on an AMD Ryzen AI MAX+ 395 (Zen 5), shared with other work, so the figures are indicative.
The requirement is at most 10 ms per second of stereo audio at 176.4 kHz with `Default`.

| Rate | Cutoff | Channels | ms CPU per s of audio | × real time |
|---|---|---|---|---|
| 88200 | Default | 1 | 2.08 | 480 |
| 88200 | Default | 2 | 4.40 | 228 |
| 88200 | Default | 6 | 13.59 | 74 |
| 88200 | Low | 1 | 2.11 | 473 |
| 88200 | Low | 2 | 4.32 | 231 |
| 88200 | Low | 6 | 13.95 | 72 |
| 176400 | Default | 1 | 3.69 | 271 |
| **176400** | **Default** | **2** | **6.78** | **147** |
| 176400 | Default | 6 | 18.46 | 54 |
| 176400 | Low | 1 | 3.08 | 325 |
| 176400 | Low | 2 | 7.12 | 140 |
| 176400 | Low | 6 | 20.06 | 50 |
| 176400 | Wide | 1 | 3.51 | 285 |
| 176400 | Wide | 2 | 7.20 | 139 |
| 176400 | Wide | 6 | 19.39 | 52 |
| 352800 | Default | 1 | 3.92 | 255 |
| 352800 | Default | 2 | 8.43 | 119 |
| 352800 | Default | 6 | 21.99 | 45 |
| 352800 | Low | 1 | 3.80 | 263 |
| 352800 | Low | 2 | 7.05 | 142 |
| 352800 | Low | 6 | 21.69 | 46 |
| 352800 | Wide | 1 | 4.42 | 226 |
| 352800 | Wide | 2 | 9.08 | 110 |
| 352800 | Wide | 6 | 27.34 | 37 |

`cargo bench -p scarletbook-pcm --bench convert` measures the same with criterion.

## Sources

General DSP literature only, recorded in the repository's CLEANROOM.md:

- Parks & McClellan 1972, and Rabiner, McClellan & Parks 1975 (Remez exchange);
- Herrmann, Rabiner & Chan 1973 (length estimates);
- Crochiere & Rabiner 1981 (multistage decimation);
- Mintzer 1982, and Vaidyanathan & Nguyen 1987 (halfband filters);
- Peled & Liu 1974, and White 1989 (bit-indexed partial-sum tables);
- Goldberg 1991 (exact integer arithmetic in IEEE-754);
- Chao et al. 1990, and Schreier & Temes 2005 (the test modulator).

## Licence

MIT OR Apache-2.0, at your option.
