# Changelog fragments

One file per pull request that changes behaviour, named after the change (e.g.
`seek-index.md` or `76-ci.md`). The first line is the CHANGELOG.md section: `Added`, `Changed`,
`Deprecated`, `Removed`, `Fixed` or `Security`. The rest is the entry, as Markdown list
item(s), with the issue and pull request numbers:

```markdown
Added
- Loudspeaker layouts of areas (specification 3, 3.4.1): ... (#75, #77).
```

`.github/scripts/fragments.py assemble VERSION DATE` folds them into CHANGELOG.md when a
release is prepared. CI checks their format, and that a pull request changing library or
program code adds one (CONTRIBUTING.md, "Change records").
