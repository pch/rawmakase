fn git_output(args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Whether the build asks Git for its commit. Release builds do, since the
/// system report and the MCP guide link name it. Development and test builds
/// don't: watching the checkout's HEAD means printing a path unique to each
/// worktree, and that output keeps a build cache from sharing the app crate
/// between worktrees.
fn reads_git() -> bool {
    std::env::var("PROFILE").as_deref() == Ok("release")
}

fn mcp_guide() {
    println!("cargo:rerun-if-changed=docs/mcp.md");
    if !reads_git() {
        // A development build links to main's guide, which never disappears.
        println!(
            "cargo:rustc-env=RAWMAKASE_MCP_GUIDE=https://github.com/pch/rawmakase/blob/main/docs/mcp.md"
        );
        return;
    }
    // Track both detached HEADs and branch updates, including linked worktrees.
    for name in [
        Some("HEAD".to_owned()),
        git_output(&["symbolic-ref", "-q", "HEAD"]),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(path) = git_output(&["rev-parse", "--git-path", &name]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    // Link to the last committed guide revision, never a feature branch that
    // may disappear. Source archives without Git use their release tag.
    let revision = git_output(&["log", "-1", "--format=%H", "--", "docs/mcp.md"])
        .filter(|s| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .unwrap_or_else(|| format!("v{}", std::env::var("CARGO_PKG_VERSION").unwrap()));
    println!(
        "cargo:rustc-env=RAWMAKASE_MCP_GUIDE=https://github.com/pch/rawmakase/blob/{revision}/docs/mcp.md"
    );
}

/// The commit and target the system report names. Only RAWmakase's own
/// checkout counts: a source archive unpacked inside another repository (a
/// packaging one) would otherwise name that repository's commit.
fn build_identity() {
    if !reads_git() {
        println!("cargo:rustc-env=RAWMAKASE_COMMIT=unknown (development build)");
        print_target();
        return;
    }
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let ours = git_output(&["rev-parse", "--show-toplevel"]).is_some_and(|top| {
        let canonical = |path: &str| std::fs::canonicalize(path).ok();
        canonical(&top).is_some_and(|top| Some(top) == canonical(&manifest))
    });
    let commit = ours
        .then(|| git_output(&["rev-parse", "HEAD"]))
        .flatten()
        .filter(|s| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .unwrap_or_else(|| "unknown (not built from a Git checkout)".into());
    println!("cargo:rustc-env=RAWMAKASE_COMMIT={commit}");
    print_target();
}

fn print_target() {
    println!(
        "cargo:rustc-env=RAWMAKASE_TARGET={}",
        std::env::var("TARGET").unwrap()
    );
}

fn main() {
    // In release builds, both follow HEAD, which mcp_guide tracks.
    mcp_guide();
    build_identity();
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=packaging/windows/rawmakase.ico");
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon("packaging/windows/rawmakase.ico")
            .set("ProductName", "RAWmakase")
            .set("FileDescription", "RAWmakase");
        if let Err(error) = resource.compile() {
            println!("cargo:warning=Windows resources not embedded: {error}");
        }
    }
}
