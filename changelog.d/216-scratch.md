Changed
- DSD-to-PCM conversion no longer zeroes its scratch buffers before the filters overwrite them: 1.5-3 % less CPU per second of audio; the output is unchanged (#216, #219).
