Changed
- Documentation: `DecodeOn::Pool` and `sb_pool_new` recommend a pool of one thread fewer than
  the logical CPUs when the calling thread does substantial work with each frame, measured
  3 to 7 % faster on 16 logical CPUs (#146, #158).
