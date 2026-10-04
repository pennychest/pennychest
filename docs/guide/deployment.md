# CI and deployment

## CI

Pull requests and pushes to `main` run:

- the backend tests
- every plugin's tests from pennychest-plugins, against this core
- a frontend build

Every push to `main` also publishes the `main` and `sha-<short sha>` [images](installation.md#the-image).

## Releases

Merging to `main` is the release. The Release workflow runs [commitizen](https://commitizen-tools.github.io/commitizen/), which reads the gitmoji on the commits since the last tag. If any of them warrant a version (see `bump_map` in `.cz.toml`), it bumps the version in `.cz.toml` and `backend/pyproject.toml`, adds an entry to `CHANGELOG.md`, commits and tags as the bot, creates a GitHub release, and publishes the image as `<version>`, `<major>.<minor>` and `latest`.

| Gitmoji | Release |
|---|---|
| 💥 | major (minor before 1.0.0) |
| ✨ 🔌 🔐 🤖 💬 📊 📈 📱 🧠 🧭 📦 | minor |
| 🐛 🚑 🩹 🔒 ⚡️ ♻️ 🗃️ 🐳 🔧 ⬆️ 💄 🍱 🚸 ♿️ 🌐 🔊 ⏪ 🗑️ 🔥 | patch |
| 📝 | none, but listed in the next release's changelog |

Anything else, such as 👷, ✅ or 🎨, merges without a release.

## Deploying

This repository doesn't deploy anything itself. To run PennyChest on a host such as Fly.io (see [Deploying to Fly.io](deploy-fly.md)), use the published image, or build your own with the [plugins](plugins.md) you want, and keep `/data` on a persistent volume.

!!! warning
    Run a single machine when using SQLite, since the database lives on one volume.
