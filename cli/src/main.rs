use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use csilctl::{color, list, send};

/// Comment for release trigger 4
/// a curl-like CLI for sending arbitrary CSIL messages
#[derive(Parser)]
#[command(name = "csilctl")]
struct Cli {
    /// path to a .csil source file
    #[arg(long, global = true)]
    client: Option<String>,

    /// disable colorized output (overridden by NO_COLOR/FORCE_COLOR env vars)
    #[arg(long, global = true)]
    disable_color: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// list the messages defined by a .csil source file
    List(ListArgs),
    /// send a message to a host via a generated client
    Send(SendArgs),
}

#[derive(Args)]
struct ListArgs {
    /// method or type name to print detail for
    item: Option<String>,
    /// print full request/response/error field detail for every message
    #[arg(long)]
    verbose: bool,
}

#[derive(Args)]
struct SendArgs {
    /// name of the message/operation to send
    #[arg(long)]
    message: String,
    /// JSON payload for the message's request fields
    #[arg(long)]
    data: Option<String>,
    /// destination address in host:port format
    #[arg(long)]
    host: String,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    color::init(cli.disable_color);

    let client = cli
        .client
        .ok_or_else(|| anyhow::anyhow!("--client is required"))?;

    match cli.command {
        Commands::List(args) => {
            let out = list::run_list(&client, args.item.as_deref(), args.verbose)?;
            print!("{out}");
        }
        Commands::Send(args) => {
            let out = send::run_send(&client, &args.message, args.data.as_deref(), &args.host)?;
            print!("{out}");
        }
    }

    Ok(())
}
