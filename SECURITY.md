# Security policy

scarletbook reads disc images that may come from anywhere, so it treats every image as
hostile: allocations are bounded, positions are checked, errors are typed, and no panic
crosses the C ABI. A way to make it crash, hang, read or write out of bounds, or allocate
without bound is a security bug.

## Supported versions

| Version | Supported |
|---|---|
| 1.0.x, the latest patch release ([releases](https://github.com/aclater/scarletbook-rs/releases)) of `scarletbook` and `scarletbook-ffi` | yes |
| `scarletbook-pcm` 0.7.0, the version 1.0.x depends on | no; frozen at 0.7.0, decommissioned ([#398](https://github.com/aclater/scarletbook-rs/issues/398)); no fixes or releases |
| `main` | yes |
| earlier 1.0.x releases, and every 0.x release | no; please check the latest 1.0.x release first |

1.0.x keeps the 0.7 interface ([docs/stability.md](docs/stability.md)), so moving from 0.7 to
1.0 needs only the version.

## Reporting a vulnerability

Please report it privately through GitHub's private vulnerability reporting:
<https://github.com/aclater/scarletbook-rs/security/advisories/new>.

Do not open a public issue or pull request for it. Include what you can of the following:
the version or commit, the image or input that triggers it (a minimal one if possible),
what happens, and what you expected. Fuzzing finds are welcome; attach the crashing input.

Once a fix is ready, it is released and the advisory is published, crediting you unless
you prefer otherwise.

## Release assets

Every release archive (the C library, the benchmark program `bench` and the benchmark kit) has
a SHA-256 checksum and a signed build provenance attestation (SLSA Build Level 2); releases
from 0.2.0 carry the attestation. For example, for `bench`:

```sh
gh attestation verify bench-<version>-x86_64-linux.tar.gz -R aclater/scarletbook-rs \
  --signer-workflow aclater/scarletbook-rs/.github/workflows/release.yml \
  --source-ref refs/tags/v<version> --deny-self-hosted-runners
```

Give both `--signer-workflow` and `--source-ref`. Without them the command accepts a file
attested by any workflow run in the repository, on any branch; with them, only one built by
the release workflow from the tag `v<version>`. `--deny-self-hosted-runners` also refuses a
file built on a self-hosted runner (#411). Check the exit status: non-zero is a failure. On a
terminal `gh` also prints the policy and `Verification succeeded!`; when its output is not a
terminal (a pipe or a script), it prints nothing on success. The same command for the C
library archives is in [docs/releases.md](docs/releases.md#verifying-a-download).

The `.sha256` file beside each archive is neither signed nor attested. It detects a damaged
download, not a replaced one: whoever could replace the archive could replace its checksum
too. Only the attestation shows who built the file.
