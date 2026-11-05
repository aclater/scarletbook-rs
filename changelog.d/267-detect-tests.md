Fixed
- Tests now fail if run-time CPU detection stops choosing the AVX2 code on a capable CPU, in `scarletbook` and `scarletbook-pcm`, or if a new `DstDecoder` or `Converter` stops using it; the bit-identity tests would still have passed (#267, #PR).
