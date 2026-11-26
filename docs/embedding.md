# RAWmakase as a library

`rawmakase-core` is RAWmakase for other apps: one crate an app links to open,
decode, edit, render and export photos as RAWmakase does, without the desktop
interface. This guide is for the programmer writing that app. How the library
is being built, and what comes next, is the [library plan](embedding-plan.md).

**Status: being built.** The library has its configuration and its error type.
Opening, decoding, rendering and exporting a photo join it in the plan's
steps 4 and 5; until then an app has nothing to do with it but build a
context. Nothing is stable before 1.0: pin a release tag.

## Depending on it

The library is not on crates.io. Depend on it by path, or by git at a
release tag (the first release that has it is still to come):

```toml
[dependencies]
rawmakase-core = { git = "https://github.com/pch/rawmakase", tag = "vX.Y.Z" }
```

That is the only RAWmakase crate an app names. The crates under it
(`rawmakase-model`, `rawmakase-decode` and the others) are how it is built;
their own public items promise nothing.

## The context

An app makes one [`Context`] when it starts and keeps it. Everything the
library needs to know about where it runs is given to the context as values:
RAWmakase's code in the library reads no environment variable and starts no
program to find it out.

```rust
use rawmakase_core::{CacheLimits, Context};

let ctx = Context::builder()
    .cache(&app.cache_dir, CacheLimits::default(), app.build_id())
    .build()?;
```

What the context takes today:

| Value | What for | Default |
| --- | --- | --- |
| The cache folder, its limits and a build id, given together | Decoded photos and AI Denoise's results are kept there, up to the limits (4 GiB and 10 GiB by default) | No cache |

The build id names the build that made a cache's entries; entries of another
id are not read. There is no default, since a default would let a newer
library read an older one's results: give one that changes whenever your app
or the library changes, such as your app's version and build number.

More values join as the library takes them (the data folders, fonts, the
GPU, the ONNX Runtime's path); a value is offered only once the library reads
it from the context.

A context is cheap to clone, and a clone is the same context. It is `Send` and
`Sync`.

## Errors

Every operation returns `rawmakase_core::Error`. Its `kind()` says what
happened, for the kinds an app treats differently:

| Kind | Means | What an app usually does |
| --- | --- | --- |
| `Cancelled` | The `Cancel` the call was given was set | Nothing: the app asked for it |
| `Unsupported` | A file or request RAWmakase doesn't handle (a CMYK JPEG) | Says so, with the message |
| `Missing` | A file, camera profile or preset that isn't there | Offers to find or install it |
| `Changed` | The file is not the one the work was made for | Opens the photo again |
| `Exists` | The destination is already there | Asks whether to replace it |
| `Invalid` | A recipe, setting or value that doesn't validate | Shows the message |
| `Other` | Anything else | Shows the message |

`{}` prints the outermost message and `{:#}` the whole chain of causes, as
the `rawmakase` command line prints its errors. `ErrorKind` is
`#[non_exhaustive]`: match it with a `_` arm.

## Threads

The API is synchronous. Long operations will take a `Cancel` and report
progress through a callback; pixel work runs on rayon's pools.

## What is shared in a process

Two contexts in one process, with different folders, each have their own
caches. What the library keeps once per process, and why sharing it can't
change a result, is listed in the plan
([What stays process-wide](embedding-plan.md#what-stays-process-wide)) and
moves here as each part is built.

## Changes

The library's public items are listed in
[`crates/rawmakase-core/public-api.txt`](../crates/rawmakase-core/public-api.txt),
which CI compares with the code. Each change to them is noted here.

- **Started**: `Context` with its cache (folder, limits, build id), `Error`
  and `ErrorKind`.
