# Contributing

Build dependencies are in the [README](README.md). The UI check needs Node 22.6 or newer:

```sh
cd app/ui && npm ci && npm run check
```

`npm run check` runs `node --experimental-strip-types` for the name-prompt tests. CI installs Node 22,
which satisfies that. `app/ui/package.json` records the same minimum in `engines`.

Before a pull request, from the repo root, with the UI already built (`npm run --prefix app/ui build`):

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
