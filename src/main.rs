mod build;
mod bundle;
mod highlight;
mod render;
mod serve;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build a self-contained static slide website.
    Build(Input),
    /// Build, preview, and reload when the outline or local images change.
    Serve {
        #[command(flatten)]
        input: Input,
        #[arg(long, default_value = "127.0.0.1")]
        host: std::net::IpAddr,
        #[arg(long, default_value_t = 3000)]
        port: u16,
        /// Open the preview in the default browser.
        #[arg(long)]
        open: bool,
    },
}

#[derive(Args, Clone)]
pub struct Input {
    /// Markdown input, relative to the working directory.
    #[arg(long, default_value = "OUTLINE.md")]
    outline: PathBuf,
    /// Generated directory, replaced in full on every successful build.
    #[arg(long, default_value = "dist")]
    output: PathBuf,
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    match Cli::parse().command {
        Command::Build(input) => {
            let mut compiler = render::Compiler::new(true)?;
            let deck = compiler.compile(&input.outline)?;
            build::publish(&deck, &input.output, &compiler.dependencies)?;
            for warning in &deck.warnings {
                eprintln!("warning: {warning}");
            }
            println!(
                "Built {} slides: {}",
                deck.slides,
                input.output.join("index.html").display()
            );
            Ok(())
        }
        Command::Serve {
            input,
            host,
            port,
            open,
        } => serve::run(input, host, port, open).await,
    }
}
