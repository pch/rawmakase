use clap::Parser;

/// Serve editing tools over MCP stdio, connected to the running desktop app.
#[derive(Parser)]
#[command(version)]
struct Args {
    #[command(flatten)]
    cli: rawmakase_mcp::Cli,
}

fn main() -> anyhow::Result<()> {
    rawmakase_mcp::run(Args::parse().cli)
}
