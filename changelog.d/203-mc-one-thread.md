Changed
- DST decoding on one thread is faster for 5 and 6 channels (and slightly for stereo), with the same output: fewer instructions per symbol, and 5 channels in 2 frames use the smaller loop body (#203, #288)
