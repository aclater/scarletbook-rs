Changed
- Reading an area no longer copies each 2048-byte audio sector out of the read buffer before demultiplexing it: a 78-minute plain-DSD stereo area reads in 0.47 s instead of 0.58 s, and writes to DSF in 1.66 s instead of 1.79 s, in generic builds on lennon (3950X) (#202, #PR).
