# Contributing

Shelfmark is currently maintained as a personal-use project. Suggestions and bug reports are welcome; code pull requests are not being accepted while the project uses its current license.

## Report a problem

- Search existing issues before opening a new one.
- Describe what you expected, what happened, and the Shelfmark version and operating system.
- Use fictional examples. Do not attach Firestorm caches, inventory exports, avatar identifiers, personal notes, photos, credentials, or machine-specific paths.
- Report a security issue privately through [GitHub Security Advisories](https://github.com/Nowalski/Shelfmark_sl/security/advisories/new).

## Run the project locally

For the maintainer’s local development workflow:

```sh
npm ci
npm run desktop:dev
```

`npm run dev` opens a browser preview with fictional sample data. It cannot access local files or connect to Firestorm. Build and release details are in [docs/BUILDING.md](docs/BUILDING.md).

Please keep cache files and local database files out of the repository. Check `git status` before committing local work.
