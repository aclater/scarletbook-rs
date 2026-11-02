Changed
- DSD-to-PCM conversion de-interleaves its input, rounds to 24 bits and interleaves its output with AVX2 when the CPU has it, chosen at run time like its filters: 3-8 % less CPU per second of audio; the output is unchanged (#216, #223).
