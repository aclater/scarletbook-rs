Changed
- `unsafe_code` is now denied in every target outside `scarletbook-ffi`, through the workspace lints and the `scarletbook-conformance` and fuzz manifests, not only in the core and PCM libraries and the `bench` binary: their integration tests, benches and examples could otherwise add `unsafe` unnoticed. No code changes; nothing was using it (#269).
