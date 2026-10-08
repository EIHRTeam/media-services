# Repository Guidelines

## Project Structure & Module Organization

`crates/` contains the Rust workspace: `xmp` models/parses/serializes metadata,
`xmp-image` rewrites JPEG/PNG/WebP/AVIF containers, `provenance` handles signing,
and `wasm` exposes bindings and fetch transport. `js/` contains the TypeScript
API, loader, batch scheduler, Node worker pool, and CLI. Rust integration tests
live in each crate's `tests/`; Node tests and the mock signer live in `test/`.
`scripts/` provides build/validation utilities; `docs/` records performance.
Edit templates in `c2pa/manifest/` and `xmp/`, then regenerate
`js/presets.ts`. Do not commit `dist/`, `pkg/`, or `target/`.

## Build, Test, and Development Commands

Use Node.js 24+, pnpm 11.26.0, Rust 1.96+, and the `wasm32-unknown-unknown`
target. Install a wasm-bindgen CLI matching `Cargo.lock`; `wasm-opt` is optional.

- `pnpm install --frozen-lockfile`: install locked dependencies.
- `pnpm build`: generate presets, compile WASM and TypeScript, and collect licenses.
- `pnpm typecheck`, `pnpm lint`, `pnpm fmt:check`: check types, Oxlint rules, and formatting.
- `pnpm fmt` and `cargo fmt --all`: format files.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: lint Rust.
- `cargo test --workspace --locked`, then `pnpm test`: run native and Node suites.
- `pnpm check:bundle` and `pnpm test:pack`: validate package contents and installed tarball behavior.

## Coding Style & Naming Conventions

Use two-space indentation for TypeScript/JavaScript and four spaces for Rust;
let Oxfmt and rustfmt enforce formatting. Use strict TypeScript and ES
modules with `.js` import suffixes. Use `camelCase` for TypeScript functions,
`PascalCase` for types, and `snake_case` for Rust functions/modules. Preserve
Node 24 and Chromium 130 compatibility when adding runtime APIs.

## Testing Guidelines

Use Rust's test harness (including Tokio async tests) and Node's `node:test` with
`node:assert/strict`. Name Node files `*.test.mjs` and Rust tests descriptively
in `crates/<crate>/tests/`. Build before Node tests; native tests produce fixtures
under `target/tmp/`. Add regression coverage for changed behavior, especially
container integrity, signature/hash validation, cancellation, and resource bounds.
No coverage threshold is configured. `pnpm acceptance` requires
c2patool and artifacts from `cargo test -p provenance --locked`.

## Commit & Pull Request Guidelines

Follow the history's `type: description` convention, using prefixes such as
`feat`, `chore`, `style`, `build`, or `test`. Keep commits focused. PRs should
describe the problem, behavior, and validation; link issues and document public
API or CLI changes. Pass applicable CI checks before review.

## Security & Configuration Tips

Keep signing keys server-side and tokens out of commits and package artifacts.
Leave `RUSTFLAGS` and `CARGO_ENCODED_RUSTFLAGS` unset for WASM builds. Preserve
the 15 MiB WASM budget and write metadata before signing to maintain asset hashes.
