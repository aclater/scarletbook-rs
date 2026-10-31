# Clean-room log fragments

One file per pull request, holding that change's row of the CLEANROOM.md Log table on a
single line, in the table's format:

```markdown
| 2026-10-31 | loudspeaker layouts (#75) | specification 3, 3.4.1 at sacd-spec `da6b900`. No new sources consulted. |
```

Every change is recorded, as CLEANROOM.md requires; the row lives here until
`.github/scripts/fragments.py assemble VERSION DATE` appends it to the Log when a release is
prepared. Sources consulted, tools installed and specification updates still go into the
other sections of CLEANROOM.md directly, in the same pull request.
