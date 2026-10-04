# CI and deployment

## CI

Pull requests and pushes to `main` run:

- the backend tests
- every plugin's tests from pennychest-plugins, against this core
- a frontend build

## Deploying

This repository doesn't deploy anything itself. To run PennyChest on a host such as Fly.io, build the image with the [plugins](plugins.md) you want and keep `/data` on a persistent volume.

!!! warning
    Run a single machine when using SQLite, since the database lives on one volume.
