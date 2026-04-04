use gateway::HotReloadGateway;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand, Debug)]
enum Commands {
    /// Start the Pulsar Gateway service
    Start {
        /// Path to the gateway configuration file
        #[arg(short, long, default_value = "gateway.yaml")]
        config: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Initialize logging
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }
    tracing_subscriber::fmt::init();

    // 2. Parse CLI Arguments
    let args = Args::parse();
 
    match args.command {
        Commands::Start { config } => {
            HotReloadGateway::start(&config).await?;
        }
    }
 
    Ok(())
}
