Added
- `scarletbook-ffi`: output option 39, `SB_OUTPUT_OPTION_DSDIFF_EMPTY_INFO` (specification 8, 8.6 row 34 at sacd-spec `048aa1a`): 0 leaves out a `DIIN` that would hold none of its chunks instead of writing it with `D = 0` (the default), as `OutputOptions::dsdiff_empty_info` does in Rust. Additive to ABI 2 (#169, #192).
