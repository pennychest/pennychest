# Signing in

The first time you open PennyChest it asks you to create a password; after that every device has to sign in.

!!! danger "Set a password straight after deploying"
    Until a password is set, anyone who reaches the page can choose one. Serve PennyChest over HTTPS when it's reachable from the internet.

## Resetting a forgotten password

Run this in the container, then open the app to choose a new one:

```bash
docker exec <container> python -m pennychest.auth.reset_password
```
