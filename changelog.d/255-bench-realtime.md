Added
- `bench realtime IMAGE --area 2ch|mc [--seeks N]`: what a real-time player needs from one thread: the mean real-time factor, the slowest 1-second window, the 99.9th percentile and slowest frame times, the frames over 6.7 ms, and the time from a seek to its first frame with a complete seek index (#255).
