Changed
- DST decoding on one thread is faster for 5 and 6 channels (and slightly for stereo), with the same output: fewer instructions per symbol, and 5 channels in 2 frames use the smaller loop body. Measured on Zen 2 only: AVX-512 builds (the `znver5` benchmark build) and Zen 5 are not measured (#203, #288)
