# CI and deployment

## CI

Pull requests and pushes to `main` run:

- the backend tests
- every plugin's tests from pennychest-plugins, against this core
- a frontend build

Every push to `main` also publishes the `main` and `sha-<short sha>` [images](installation.md#the-image).

## Releases

Merging to `main` is the release. The Release workflow runs [commitizen](https://commitizen-tools.github.io/commitizen/), which reads the gitmoji on the commits since the last tag. If any of them warrant a version (see `bump_map` in `.cz.toml`), it bumps the version in `.cz.toml` and `backend/pyproject.toml`, adds an entry to `CHANGELOG.md`, commits and tags, and creates a GitHub release. The tag then publishes the image as `<version>`, `<major>.<minor>` and `latest`.

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
- **Installed on:** pennychest, pennychest-plugins and pennychest.github.io.
- **In the organisation's Actions settings:** the app's client ID as the `RELEASE_APP_CLIENT_ID` variable, and a private key generated for it as the `RELEASE_APP_PRIVATE_KEY` secret, both available to pennychest and pennychest-plugins.
- **In the `main` ruleset** of pennychest and pennychest-plugins: the app added to the bypass list, set to Always.

## Deploying

This repository doesn't deploy anything itself. To run PennyChest on a host such as Fly.io (see [Deploying to Fly.io](deploy-fly.md)), use the published image, or build your own with the [plugins](plugins.md) you want, and keep `/data` on a persistent volume.

!!! warning
    Run a single machine when using SQLite, since the database lives on one volume.
