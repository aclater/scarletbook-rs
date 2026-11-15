# Security policy

scarletbook reads disc images that may come from anywhere, so it treats every image as
hostile: allocations are bounded, positions are checked, errors are typed, and no panic
crosses the C ABI. A way to make it crash, hang, read or write out of bounds, or allocate
without bound is a security bug.

## Supported versions

| Version | Supported |
|---|---|
| the latest release ([releases](https://github.com/aclater/scarletbook-rs/releases)) | yes |
| `main` | yes |
| older releases | no; please check the latest release first |

## Reporting a vulnerability

Please report it privately through GitHub's private vulnerability reporting:
<https://github.com/aclater/scarletbook-rs/security/advisories/new>.

Do not open a public issue or pull request for it. Include what you can of the following:
the version or commit, the image or input that triggers it (a minimal one if possible),
what happens, and what you expected. Fuzzing finds are welcome; attach the crashing input.

Once a fix is ready, it is released and the advisory is published, crediting you unless
you prefer otherwise.

## Release assets

Release binaries carry a SHA-256 checksum and, from 0.2.0, a signed build provenance
attestation (SLSA Build Level 2), which can be checked with:

```sh
gh attestation verify bench-<version>-x86_64-linux.tar.gz -R aclater/scarletbook-rs \
  --signer-workflow aclater/scarletbook-rs/.github/workflows/release.yml \
  --source-ref refs/tags/v<version>
```

Give both `--signer-workflow` and `--source-ref`. Without them the command accepts a file
attested by any workflow run in the repository, on any branch; with them, only one built by
the release workflow from the tag `v<version>`.

The `.sha256` file beside each archive is neither signed nor attested. It detects a damaged
download, not a replaced one: whoever could replace the archive could replace its checksum
too. Only the attestation shows who built the file.
