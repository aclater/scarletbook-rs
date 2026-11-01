Changed
- Provisional (cold) seeks take at most 5 read calls and 160 KiB up to the range's first frame on
  the 50-minute 2-channel DST area of uneven bit rate, at every one of 2 000 (and 30 000)
  positions (#123): when the search reaches its fifth read, that read takes what is left of
  160 KiB around its guess (or on from where a final read starts, past the probed bound), so
  that it ends the search and holds the first frame at once; if it finds no sector before the
  frame, the search goes on as before. Before, 21 of 1 991 positions took 6 or 7 calls (all
  within 144 KiB); now p50 / p90 / p95 / p99 / max are 3 / 4 / 5 / 5 / 5 calls and
  122 / 144 / 144 / 160 / 160 KiB. The constant-rate image is unchanged (#123, #PR).
