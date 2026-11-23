Changed
- A new `DstDecoder`, or a frame with more filters or ptables than any before it, now writes each new byte table and probability table once, rather than filling it with zeros first and then computing over the zeros. Output unchanged (#383, #392).
