Added
- C interface: `sb_build_info()`, a static string of how the library was built (version, target, profile and optimisation level, enabled features, target CPU features, compiler, commit), recorded by a build script of the FFI crate; the release smoke test checks that it names the package's target and `sb_version()`'s version (#108).
