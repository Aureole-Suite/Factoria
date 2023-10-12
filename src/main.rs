#![feature(try_blocks)]
use clap::Parser;
use eyre_span::emit;

mod util;
mod grid;
mod dirdat;

mod command;

fn main() -> eyre::Result<()> {
	use tracing_error::ErrorLayer;
	use tracing_subscriber::prelude::*;
	use tracing_subscriber::{fmt, EnvFilter};

	let fmt_layer = fmt::layer().with_target(false);
	let filter_layer = EnvFilter::try_from_default_env()
		.or_else(|_| EnvFilter::try_new("info"))?;

	tracing_subscriber::registry()
		.with(filter_layer)
		.with(fmt_layer)
		.with(ErrorLayer::default())
		.init();

	eyre_span::install()?;

	let cli = command::Cli::parse();
	emit(command::run(cli));

	Ok(())
}
