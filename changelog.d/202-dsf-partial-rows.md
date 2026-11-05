Changed
- DSF output fills the rows that a read only touches, at the ends of each `Layout::fill` or `Layout::read_at` call, whole in a scratch row and copies from it, instead of a byte at a time. The output is unchanged. Plain-DSD areas to DSF take 2 to 5 % less time with 4 MiB calls and 28 to 51 % less with 128 KiB calls (#202, #PR).
