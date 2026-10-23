# spargebra 0.4.7, unpatched

This folder holds spargebra 0.4.7 as published on crates.io (oxigraph commit
in `.cargo_vcs_info.json`, `lib/spargebra`), with no change. Both
`Cargo.toml` files use it through `[patch.crates-io]`. spargebra is licensed
MIT OR Apache-2.0; we use it under MIT (`LICENSE-MIT`, copied from
`vendor/spareval`, which has the same oxigraph licence text).

## Patches

None yet. `tools/check-vendor.sh` checks that this folder equals the
published crate.

## Review rule for a patch

To be written by the ticket that adds the first patch (ticket 07).

## Removing the patches

Once a spargebra release has every change, delete this folder and the
`[patch.crates-io]` entries in `Cargo.toml` and `web/engine/Cargo.toml`, then
run `cargo update -p spargebra` in both places.
