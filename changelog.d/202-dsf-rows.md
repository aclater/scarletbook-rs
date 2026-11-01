Changed
- DSF output fills whole rows a position at a time, separating every channel in one pass with a loop per channel count: a plain-DSD stereo area to DSF takes 1.10 s instead of 2.86 s in x86-64-v3 builds and 1.93 s in generic ones (to `/dev/null`); the bytes are unchanged (#202, #PR).
