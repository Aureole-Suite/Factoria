pub mod extract;
pub mod list;
pub mod add;
pub mod remove;
pub mod rebuild;
pub mod index;
pub mod create;

#[derive(Debug, Clone, clap::Parser)]
#[command(args_conflicts_with_subcommands = true, disable_help_subcommand = true)]
pub struct Cli {
	#[clap(subcommand)]
	command: Option<Command>,
	#[clap(flatten)]
	extract: Option<extract::Command>,
}

#[derive(Debug, Clone, clap::Subcommand)]
pub enum Command {
	/// Extract files from archives [default]
	Extract(extract::Command),
	/// List files in archives [ls]
	#[clap(alias = "ls")]
	List(list::Command),
	/// Add or replaces files to archives
	Add(add::Command),
	/// Delete files from archives [rm]
	#[clap(alias = "rm")]
	Remove(remove::Command),
	/// Clear out unused data from archives
	Rebuild(rebuild::Command),
	/// Create a json index file for an archive
	Index(index::Command),
	/// Create an brand new archive from scratch a json index file
	Create(create::Command),
}

pub fn run(cli: Cli) -> eyre::Result<()> {
	let command = cli.command.or(cli.extract.map(Command::Extract)).expect("no command");
	match command {
		Command::Extract(cmd) => extract::run(&cmd),
		Command::List(cmd) => list::run(&cmd),
		Command::Add(cmd) => add::run(&cmd),
		Command::Remove(cmd) => remove::run(&cmd),
		Command::Rebuild(cmd) => rebuild::run(&cmd),
		Command::Index(cmd) => index::run(&cmd),
		Command::Create(cmd) => create::run(&cmd),
	}
}
