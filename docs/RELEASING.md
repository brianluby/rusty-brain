# Releasing rusty-brain

Releases are tag-triggered and fail closed before publication. The release
workflow requires the tag version to match `[workspace.package].version`, a
dated or undated released section for that version in `CHANGELOG.md`, and the
tagged commit to be reachable from `main`.

## Prepare and publish

1. Update `[workspace.package].version` in `Cargo.toml` and refresh the lockfile.
2. Move the release notes out of `[Unreleased]` into
   `## [X.Y.Z] - YYYY-MM-DD`, leaving a new `[Unreleased]` section behind.
3. Merge the release commit to `main`, then create an annotated `vX.Y.Z` tag
   at that commit and push the tag.
4. Wait for every preflight, build, and packaged-artifact smoke job to pass.
   Publication happens only after all three native targets prove auto-start,
   remember/recall, owner-only permissions, and capture-time redaction.
5. Verify the release contains all three tarballs plus `SHA256SUMS`. Build
   provenance attestations remain attached to each tarball.

You can run the deterministic checks before tagging:

```bash
scripts/release-preflight.sh --tag vX.Y.Z --commit HEAD --main-ref origin/main
```

Before creating a tag, run the same three-target build and packaged-smoke matrix
without publishing:

```bash
gh workflow run release.yml --ref <verification-branch>
```

Manual runs name artifacts by the full source SHA, exercise the deterministic
preflight self-tests, and build, attest, checksum and smoke each native package.
They never create a release and do not certify tag/version/changelog/ancestry
alignment; actual tag publication still requires those checks. The first complete
manual matrix passed on ARM macOS, Intel macOS and Linux at `2cb6acf` over the
merged `a33ea3b` product source:
[native verification run](https://github.com/brianluby/rusty-brain/actions/runs/38924475235).

## Check a local packaged artifact

The tarball must contain one top-level directory with executable
`rusty-brain`, `rusty-brain-hooks` and `rusty-brain-install` binaries for the
machine running the check:

```bash
scripts/verify-fresh-install.sh --tarball dist/rusty-brain-VERSION-TARGET.tar.gz
bash scripts/verify-fresh-install.test.sh --tarball dist/rusty-brain-VERSION-TARGET.tar.gz
```

Relative paths resolve against the invoking directory; absolute paths and spaces
are supported. An adjacent `SHA256SUMS` verifies the private copy that is unpacked,
not a mutable original. Without that file, local mode explicitly skips checksums;
this is not release-integrity evidence. Tagged downloads still require checksums.

The regression harness exercises real packaged executables: valid paths,
missing/unreadable inputs, bad checksums, source replacement after verification,
and a damaged hook failing after daemon auto-start. A failed checklist stops its
owned daemon and retains diagnostics; a failed harness also retains its fixtures.
Successful runs remove disposable HOME/config/store/socket data.

Run on the native target. One ARM macOS pass does not certify Intel macOS or
Linux; the release workflow still requires all three native jobs before publish.

## Failure and rollback

The build matrix uses `fail-fast: false`, so a partial failure preserves the
other jobs' diagnostics, but no GitHub release is published until every target
and native smoke test passes. Fix the failure on `main`; do not publish the
partial artifacts manually.

If the tag-triggered workflow failed before a GitHub release was published,
delete the failed remote tag, recreate it at the corrected commit on `main`,
and push it again:

```bash
git push origin :refs/tags/vX.Y.Z
git tag -d vX.Y.Z
git tag -a vX.Y.Z -m "rusty-brain vX.Y.Z" <corrected-main-commit>
git push origin vX.Y.Z
```

If a release was published, never move or reuse its tag. Mark the bad release
as deprecated, fix forward, increment the patch version, add a new changelog
section, and publish a new tag. Generated checksums and provenance belong to
the exact artifacts they describe and must not be copied to a replacement.
