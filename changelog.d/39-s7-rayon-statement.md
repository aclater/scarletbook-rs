Changed
- The documentation of `DecodeOn` and the README now state that, with the `parallel` feature, rayon 1 is part of the public Rust API by decision: `DecodeOn::Pool` keeps taking an `Arc<rayon::ThreadPool>`, and a rayon 2 would need a new major version of `scarletbook`. Documentation only: nothing breaks and no code changes (#39, #292).
