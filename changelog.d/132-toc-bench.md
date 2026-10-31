Added
- A criterion benchmark of opening a disc and reading its whole TOC, through the C
  interface (`sb_disc_open_callbacks_at` and every TOC query) and through the Rust API:
  `cargo bench -p scarletbook-ffi --bench toc_read` (#132, #135).
