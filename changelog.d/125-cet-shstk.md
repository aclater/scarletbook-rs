Added
- The x86-64 Linux C libraries (`libscarletbook_ffi.so` and every object in
  `libscarletbook_ffi.a`, glibc and musl) carry the Intel CET shadow stack (SHSTK) property,
  so a SHSTK-marked program that links them stays marked and glibc with shadow stacks enabled
  loads them; `clib-test.sh` checks it with `readelf -n`. Indirect branch tracking (IBT) is
  not marked: stable Rust's standard library has no `ENDBR64` (#125, #PR).
