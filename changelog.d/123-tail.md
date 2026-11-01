Changed
- Provisional (cold) seeks keep within about 144 KiB before the range's first frame, and take
  fewer read calls at the worst positions (#123): the search's first correction uses the
  geometric mean of the probe's own density and the average back to the other track start;
  once probes lie on both sides of the frame it uses regula falsi with the Illinois weighting;
  it reads a narrow enough interval at once; and the read after the provisional start is sized
  through the range's first frame and on toward the range's end within that budget, less than
  32 sectors when that suffices, then on to the range's end as before. On a 50-minute
  2-channel DST area of uneven bit rate (2 000 positions, to the first frame) the worst case
  goes from 8 read calls and 284 KiB to 7 and 144 KiB (99th percentile 7 → 6 calls, 206 → 144
  KiB); the median rises from 48 to 122 KiB, since the range's first frame now comes with the
  read that also takes the rest of the range (the whole range: 116 → 128 KiB, 4 calls).
  Reads sized after a resume from a seek index are now also exactly their size (#123, #159).
