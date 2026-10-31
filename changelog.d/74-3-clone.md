Added
- `sb_disc_clone` and `sb_disc_clone_with_log` (#74): a clone of a disc opened from a file or from memory, with a reading position of its own, for reads of one image on several threads at once; track and raw frames handles may be read and continued through any clone of their disc. Discs opened from a file now read it at offsets through one shared handle. Additive: the ABI version stays 1.
