# Nix Build Fixes for cross-chain-redesign

## Status
In progress. Two of three issues fixed, third partially solved.

## Issues

### 1. cargo-deny advisory failures (FIXED)
**File:** `deny.toml`

Added ignores for 5 unmaintained transitive deps from sp1-sdk:
- `RUSTSEC-2024-0384` — `instant` (via `backoff`)
- `RUSTSEC-2021-0139` — `ansi_term` (via `tracing-forest`)
- `RUSTSEC-2025-0012` — `backoff`
- `RUSTSEC-2025-0141` — `bincode` (via `sp1-core-executor`)
- `RUSTSEC-2024-0388` — `derivative` (via `ark-ff`)

Also: local `cargo-deny` needed update from 0.18.2 to 0.19.0 (CVSS 4.0 support).

### 2. sp1 cbindgen cargo metadata failures (FIXED)
**File:** `flake.nix` — `overrideVendorCargoPackage`

**Root cause:** `sp1-core-machine` and `sp1-recursion-core` have build.rs files that use `cbindgen`, which calls `cargo metadata --all-features` on the vendored crate. This fails because:

1. **Dev-deps not vendored:** `criterion`, `rand`, etc. are dev-dependencies that crane doesn't vendor, but `cargo metadata` tries to resolve them.
2. **Optional deps not vendored:** `--all-features` activates `bigint-rug` feature on `sp1-curves`, pulling in `rug` which isn't vendored.
3. **Stale Cargo.lock:** The shipped Cargo.lock has version mismatches (e.g., `cfg-if 1.0.0` vs vendored `1.0.4`).
4. **Read-only nix store:** `cargo metadata` tries to write a new Cargo.lock but the vendored crate dir is in `/nix/store` (immutable).

**Solution applied (working):**
- Strip `[dev-dependencies.*]` sections from Cargo.toml: `sed -i '/^\[dev-dependencies/,/^$/{d;}'`
- Remove stale Cargo.lock: `rm -f Cargo.lock`
- Patch build.rs to copy crate dir to writable tmpdir before cbindgen runs (inserts Rust code after `let crate_dir = PathBuf` that recursively copies to `std::env::temp_dir()`)
- Override `sp1-curves` to remove optional `rug` dep and empty the `bigint-rug` feature

**What we tried that didn't work:**
- `chmod -R u+w` on copied vendor dir — nix sandbox still prevents writes
- `cp -r --no-preserve=mode` — same issue
- `rsync --chmod=u+w` — same issue
- `tar | tar` to strip permissions — same issue (all in preBuild, writing to $TMPDIR)
- Generating a lockfile locally and embedding it — version mismatches (92 crates differ between local crates.io and vendored versions)
- Sed-fixing the lockfile — too many mismatches to fix individually

**Key insight:** The nix build sandbox on macOS prevents writing to files in `$TMPDIR` that originated from `/nix/store`, regardless of permissions. The only reliable approach was patching the build.rs source to copy to a truly writable temp location at Rust runtime (not shell time).

### 3. ibc-eureka-solidity-types sol!() macro paths (IN PROGRESS)
**File:** `flake.nix` — wrapper derivation around `cargoVendorDir`

**Root cause:** `ibc-eureka-solidity-types` crate uses `alloy_sol_types::sol!("../../contracts/...")` and `sol!("../../abi/...")` relative paths from `src/msgs.rs`. In the original monorepo, `../../` from `packages/solidity/src/` reaches the repo root where `contracts/` and `abi/` live.

In crane's vendor layout:
```
vendor-cargo-deps/
  d527247c.../                    # symlink -> linkLockedDeps
    ibc-eureka-solidity-types-0.1.0/  # symlink -> git checkout
      src/msgs.rs                 # sol!("../../contracts/...")
                                  # resolves to d527247c.../contracts/
  config.toml
```

The `../../contracts/` from `src/msgs.rs` resolves to the git checkout hash directory level. We need `contracts/` and `abi/` as siblings of the crate dirs inside that hash directory.

**What we tried:**
1. `overrideVendorGitCheckout` to copy contracts into checkout — puts them at wrong level (inside the checkout, but the checkout is one more symlink hop away)
2. Wrapper derivation with `cp -r . $out` — doesn't dereference symlinks
3. Wrapper with `cp -rL` — macOS cp -rL doesn't fully dereference nested symlinks in nix store
4. Wrapper with `rsync -rL` — permission denied creating dirs inside nix store targets
5. Wrapper with `tar -ch | tar -x` — current attempt, not yet verified

**The fundamental challenge:** The vendor-cargo-deps directory is a tree of symlinks pointing to immutable nix store paths. To inject files at the right level, we need to materialize (dereference) the entire tree into a writable copy, then add files. `tar -chf` (dereference symlinks) should work but hasn't been verified yet.

**Latest result (tar -ch approach):**
The `tar -chf | tar -xf` wrapper derivation DOES work to materialize the vendor dir — it got past the sol!() errors. However, it then fails with:
```
error: cannot update the lock file Cargo.lock because --locked was passed
```
The materialized vendor dir has different paths than the original symlinked one, so the `config.toml` source-replacement checksums don't match. The `config.toml` from crane references the original nix store paths, but after materialization the files live at new paths.

**Fix needed:** After tar-copying, also update `config.toml` to replace original nix store paths with the new `$out` paths. Or regenerate checksums.

**Alternative approaches not yet tried:**
- Instead of materializing the whole vendor dir, only materialize the git checkout hash dir that needs contracts/ injected (leave registry crates as symlinks)
- Use `overrideVendorGitCheckout` more carefully — the checkout root IS the dir containing crate dirs, so `contracts/` should go there. The issue was that `cp -r` into a read-only nix store output failed. Try using the checkout override's `postPatch` which runs in a writable build dir.
- Patch the sol!() macro paths in the vendored source to use absolute nix store paths via sed
- Use crane's `preBuild` hook to symlink contracts at the right relative path from the cargo build dir at build time (not vendor time)
