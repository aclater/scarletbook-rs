Changed
- 5-channel DSF output is faster: the channels of a 5-channel row are separated four frames at a time in 32-bit words, which the compiler vectorises, where it had been inserting one byte at a time. A 30-minute plain-DSD 5-channel area writes to DSF in 0.67 s instead of 0.92 s on one thread in generic builds on lennon (3950X). The output is unchanged (#202, #PR).
