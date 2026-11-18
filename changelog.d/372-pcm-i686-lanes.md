Changed
- DSD-to-PCM conversion on 32-bit x86 (i686): the Fir1 stage computes 8 outputs at a time instead of 16, which fits SSE2's 8 vector registers there, so 176.4 and 352.8 kHz output take about 4 to 6 % less time. The output is identical to the bit. CI now also tests `scarletbook-pcm` on i686 (#372).
