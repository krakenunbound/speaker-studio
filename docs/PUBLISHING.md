# Publishing Speaker Studio

This is a Windows source project with MIT licensing. Preparing these files does not create a GitHub repository or upload anything.

## Before the first push

1. Run the checks in [CONTRIBUTING.md](../CONTRIBUTING.md).
2. Review `git status --short`, `git diff --cached`, and `git ls-files`. Only intended source, tests, icons, and documentation should be included.
3. Review the existing Git history as well as the current files for private data. Ignoring a path does not remove it from earlier commits.
4. Confirm that the [MIT license](../LICENSE), app name, artwork, and commit author details are suitable for public release.
5. Create an empty GitHub repository with the desired visibility. Leave its automatic README, license, and ignore-file initialization unchecked because those files already exist here.

After replacing `YOUR-REPOSITORY-URL` with the URL of that repository:

```powershell
git remote add origin YOUR-REPOSITORY-URL
git push -u origin main
```

If `origin` already exists, inspect it with `git remote -v` rather than adding it again. The GitHub Actions checks will run after the push; inspect their results before creating a release.

## Versioned releases

Keep `Cargo.toml`, `VERSION`, the changelog, and version references in the guides consistent. Update the package entry in `Cargo.lock` when changing the Cargo version, then run the checks and rebuild. Commit the release changes before creating a version tag. Existing tags identify earlier snapshots; do not move them to include later housekeeping.

Push only the intended release tag using `git push origin TAG-NAME`, replacing `TAG-NAME` with that tag. Create a GitHub release from that tag and describe the changes and setup requirements.

For the simplest distribution, use GitHub's source archive and the documented first-run setup. Do not upload the whole working folder: it contains local dependencies and may contain recordings. A downloadable `Speaker Studio.exe` by itself is incomplete; the Python engine and its dependencies are still required. If supplying a prebuilt executable, include the matching source/setup files, user guide, and license, and explain the remaining setup requirements.

Do not redistribute model weights or third-party runtime packages without checking their own licenses and packaging requirements. Keep release archives in the ignored `dist/` folder when preparing them locally.

## Local cleanup

`target/` and Python `__pycache__/` directories are disposable build caches. Removing `target/` means the next Rust build will compile dependencies again. Preserve `.engine/` for the installed speech engine, `sessions/` for recordings, and `Speaker Studio.exe` for normal launches. Model caches may also exist outside this project in the user's Hugging Face cache.
