Added
- Loudspeaker layouts through the C interface: `sb_area_loudspeaker_layout` gives an area's `SB_LAYOUT_*` (stereo, 3/2, 5.1, or `SB_LAYOUT_UNSPECIFIED` for a combination specification 3, 3.4.1 does not define) and `sb_area_channel_speaker` the `SB_SPEAKER_*` of each channel in decoded-frame order (`SB_SPEAKER_UNSPECIFIED` without a defined layout). Additive: the ABI version stays 1 (#75).
