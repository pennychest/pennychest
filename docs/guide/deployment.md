# CI and releases

## CI

Pull requests and pushes to `main` run:

- `cargo fmt`, clippy and the tests
- the frontend's lint and build
- a build of these docs

Every push to `main` also publishes the `main` and `sha-<short sha>` [images](installation.md#the-image).

## Releases

Merging to `main` is the release. The Release workflow runs [commitizen](https://commitizen-tools.github.io/commitizen/), which reads the gitmoji on the commits since the last tag. If any of them warrant a version (see `bump_map` in `.cz.toml`), it bumps the version in the workspace `Cargo.toml` (and the workspace's crates in `Cargo.lock`), adds an entry to `CHANGELOG.md`, commits and tags, and creates a GitHub release. The tag then publishes the image, for amd64 and arm64, as `<version>`, `<major>.<minor>` and `latest`. `/api/health` reports the version that's running.

| Gitmoji | Release |
|---|---|
| 💥 | major (minor before 1.0.0) |
| ✨ 🔌 🔐 🤖 💬 📊 📈 📱 🧠 🧭 📦 | minor |
| 🐛 🚑 🩹 🔒 ⚡️ ♻️ 🗃️ 🐳 🔧 ⬆️ 💄 🍱 🚸 ♿️ 🌐 🔊 ⏪ 🗑️ 🔥 | patch |
| 📝 | none, but listed in the next release's changelog |

Anything else, such as 👷, ✅ or 🎨, merges without a release.

### The release app

`main` only accepts changes through pull requests, so the Release workflow pushes the bump commit and tag as a GitHub App that the `main` ruleset lets through. `GITHUB_TOKEN` can't be given that bypass. The Docs workflow uses the same app to ask pennychest.github.io to rebuild. The app needs:

- **Repository permissions:** Contents read and write. Metadata read-only is added automatically.
- **Installed on:** this repository and pennychest.github.io.
- **In the organisation's Actions settings:** the app's client ID as the `RELEASE_APP_CLIENT_ID` variable, and a private key generated for it as the `RELEASE_APP_PRIVATE_KEY` secret, both available to this repository.
- **In the `main` ruleset:** the app added to the bypass list, set to Always.

## Deploying

This repository doesn't deploy anything itself. To run PennyChest on a host (see [Quick start](quick-start.md)), use the published image and keep `/data` on a persistent volume.

!!! warning
    Run a single machine, since the database lives on one volume.
