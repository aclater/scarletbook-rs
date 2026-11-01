Changed
- DSD-to-PCM conversion sums the first stage's table lookups with the table count known at compile time: about 10 % less CPU per second of audio at 176.4 kHz and 88.2 kHz, 3-7 % at 352.8 kHz; the output is unchanged (#216, #217).
