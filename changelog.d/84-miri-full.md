Changed
- CI: the long Miri slices (core unit tests, fast-loop checks, long parallel-path tests) move to `miri-full.yml`, which runs on every push to main, nightly and on demand, and queues instead of cancelling, so later merges can no longer keep them from completing (#84).
