Changed
- A disc of a raw image (2064 or 2054 bytes per stored sector) no longer keeps the read buffer of its TOC reads once opened or shared: it held the largest TOC read, 20 KiB or more per disc (about 80 KiB on the discs of #228), for as long as the disc was kept. The first audio read allocates it again; the output is unchanged (#228, #234).
