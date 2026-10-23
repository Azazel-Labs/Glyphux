# Releasing Glyphux

The CI workflow in `.github/workflows/ci.yml` releases tags matching `v*`.
A tag must exactly match the workspace version, for example `v0.3.0`.

## Setup

Add a crates.io API token as the repository Actions secret
`CARGO_REGISTRY_TOKEN`. It needs permission to publish `glyphux`,
`glyphux-atlas`, `glyphux-gpu` and `glyphux-cli`.

## Release

1. Update the workspace version and internal dependency versions in `Cargo.toml`,
   plus the standalone web demo version in `examples/web-demo/Cargo.toml`.
2. Update both lockfiles, validate the changes, and commit them.
3. Create an annotated tag and push the commit and tag together:

   ```sh
   git tag -a v0.3.0 -m "Glyphux 0.3.0"
   git push --atomic origin main v0.3.0
   ```

The tag workflow checks versions, formatting, lints, tests, minimum Rust versions
and the web demo build. After those pass, it creates the GitHub Release, verifies
and publishes the four crates, and attaches offline CLI ZIPs for Linux, Windows
and macOS. Cargo handles dependency ordering. The token is used only in the
publishing step.

The web demo and examples have `publish = false` and are not in the publishing
package list. Only pushes to `main` deploy the web demo to GitHub Pages.
Publishing a GitHub Release manually does not start a second publishing run.

## Recovery

If the API token is missing, add the secret and rerun the failed publishing job
from GitHub Actions. Do not move an existing release tag. If an upload partially
succeeds, check crates.io first: published versions cannot be overwritten. Publish
only the remaining packages at that version, or prepare a new version.
