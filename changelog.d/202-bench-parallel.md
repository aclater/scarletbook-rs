Added
- `bench dsf --via parallel`: one thread reads the area while `--threads` workers fill whole DSF rows and write them at their offsets, a prototype of a parallel DSF writer on the public `Layout` API (#202, #224, #232).
