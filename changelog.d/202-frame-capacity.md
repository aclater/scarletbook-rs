Changed
- A plain DSD frame is assembled in a buffer of its final size, `C x 4704` bytes, reserved when it starts, instead of one grown sector by sector: reading a plain-DSD area takes 0.71 s instead of 0.85 s for 78 minutes of stereo (#202, #PR).
