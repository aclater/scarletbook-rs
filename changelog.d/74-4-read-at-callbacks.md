Added
- `sb_disc_open_callbacks_at[_with_log]` and `sb_disc_clone_callbacks[_with_log]` (#74): discs read through a positional read callback (`SbReadAtFn`, which may be called from several threads at once), which can be cloned, and clones of any disc through new read and seek callbacks. Additive: the ABI version stays 1.
