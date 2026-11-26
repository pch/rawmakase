# rawmakase-embed

RAWmakase's library ([`rawmakase-core`](../../crates/rawmakase-core), its
[guide](../../docs/embedding.md)) used from Rust with no interface, as an app
outside RAWmakase's workspace uses it: a workspace of its own, with its own
lock file, naming one RAWmakase crate.

```sh
cargo run -- photo.dng out.jpg 0.5   # open, Exposure +0.5, export
cargo test                           # every chart DNG in tests/corpus/charts
cargo test -- --include-ignored      # and the desktop's pixels, once
                                     # `cargo build --bin rawmakase` has run
                                     # in the repository
```

The ignored test is the library plan's fourth measure: the library and
`rawmakase render` export the same photo with the same saved recipe, each with
an empty data folder of its own, to a 16-bit TIFF, and the pixels are equal.
