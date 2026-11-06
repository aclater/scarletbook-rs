Fixed
- C header documentation: the conventions now state how each kind of call reports the buffer size it needs, what a `const` handle pointer does and does not promise, and all twelve opaque handles. `sb_track_audio_read` and `sb_dst_decode` now state what `SB_STATUS_BUFFER_TOO_SMALL` does; `sb_dst_decode`'s `out` may be NULL. No function's behaviour changes (#39, #322).
