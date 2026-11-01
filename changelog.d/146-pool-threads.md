Changed
- Documentation: `DecodeOn::Pool` and `sb_pool_new` recommend a pool of one thread fewer than
  the logical CPUs when the calling thread does substantial work with each frame. Measured on
  16 vCPUs, decoding through the C interface and writing each frame to files took 7.7 % longer
  than `bench decode` with 16 threads and 4.6 % longer with 15 (about 3 % faster); with a
  caller that does little, 16 were as fast or slightly faster. Measure with `decode_pool`
  (#146, #158).
