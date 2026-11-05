Added
- `bench open` (open to parsed TOC) and `bench first` (first bytes of a track from a seek), timed inside the process, in the page cache or, with `--evict`, with the image evicted before each iteration. `benchmarks/evict.py` evicts without root and fails if any page stays cached; the run list gains warm and cold cases for both, and `table.py` shows their medians in microseconds (#224).
