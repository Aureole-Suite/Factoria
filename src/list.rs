use std::path::{PathBuf, Path};

use clap::ValueHint;
use crate::grid::{Grid, Columns, Cell, Orientation};

use bzip::CompressMode;
use themelios_archive::dirdat::{self, DirEntry};

use crate::util::mmap;

#[derive(Debug, Clone, clap::Args)]
pub struct List {
	/// Include zero-sized files
	#[clap(short, long)]
	all: bool,
	/// Filter which files to include
	#[clap(short, long)]
	glob: Vec<String>,

	/// Show a detailed view with one file per line
	#[clap(short, long)]
	long: bool,
	/// Show one file per line
	#[clap(short='1', long, overrides_with("long"))]
	oneline: bool,
	/// Show several files per line (default)
	#[clap(short='G', long, overrides_with("long"), overrides_with("oneline"))]
	grid: bool,
	/// Draws grid left to right, not downwards
	#[clap(short='x', long)]
	across: bool,

	/// Use binary prefixes instead of SI for file sizes
	#[clap(short, long)]
	binary: bool,
	/// Display raw number of bytes for file sizes
	#[clap(short='B', long, overrides_with("binary"))]
	bytes: bool,

	/// Do not attempt to decompress files
	#[clap(short='C', long)]
	compressed: bool,

	/// Show (decompressed) file size in short modes
	#[clap(short='S', long)]
	size: bool,

	/// Show file id in short modes
	#[clap(short, long)]
	id: bool,

	/// Show compressed size in addition to decompressed size
	#[clap(short='c', long)]
	compressed_size: bool,

	/// Specify sort order
	#[clap(short, long, default_value="id", require_equals(true), num_args=0..=1, default_missing_value="name")]
	sort: SortColumn,
	/// Reverse sort order
	#[clap(short, long)]
	reverse: bool,

	/// The .dir file(s) to inspect.
	#[clap(value_hint = ValueHint::FilePath, required = true)]
	dir_file: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, clap::ValueEnum)]
pub enum SortColumn {
	Id,
	Name,
	Size,
	CSize,
	Time,
}

#[derive(Debug)]
pub struct Entry {
	dirent: DirEntry,
	index: u16,
	decompressed_size: Option<usize>,
	compression_mode: Option<bzip::CompressMode>,
}

impl std::ops::Deref for Entry {
	type Target = DirEntry;

	fn deref(&self) -> &Self::Target {
		&self.dirent
	}
}

pub fn run(cmd: &List) -> eyre::Result<()> {
	println!("{:?}", cmd);
	for (idx, dir_file) in cmd.dir_file.iter().enumerate() {
		if cmd.dir_file.len() != 1 {
			println!("{}:", dir_file.display());
		}

		list_one(cmd, dir_file)?;

		if idx + 1 != cmd.dir_file.len() {
			println!();
		}
	}
	Ok(())
}

fn list_one(cmd: &List, dir_file: &Path) -> eyre::Result<()> {
	let archive_number = get_archive_number(dir_file);
	let entries = get_entries(cmd, dir_file)?;
	if cmd.long {
		todo!();
	} else {
		let mut cells = Vec::new();

		for e in &entries {
			format_entry_short(cmd, archive_number, e, &mut cells);
		}

		print_grid(cmd, usize::from(cmd.size) + usize::from(cmd.id) + 1, cells);
	}
	Ok(())
}

fn format_entry_short(cmd: &List, archive_number: Option<u8>, e: &Entry, cells: &mut Vec<Cell>) {
	// ls puts inode before size, but since id is fixed size it looks better in the middle
	if cmd.size {
		cells.push(Cell::right(format_size(cmd, e)));
	}
	if cmd.id {
		let mut s = String::new();
		if let Some(archive_number) = archive_number {
			s.push_str(&format!("{:02X}", archive_number));
		}
		s.push_str(&format!("{:04X}", e.index));
		cells.push(Cell::left(s));
	}
	cells.push(Cell::left(format_name(cmd, e)));
}

fn format_name(_cmd: &List, e: &Entry) -> String {
	let mut s = String::new();
	let ext = e.name.split_once('.').map_or("", |a| a.1);
	if let Some(color) = get_color(ext) {
		s.push_str(&format!("\x1B[38;5;{color}m"))
	}
	if e.timestamp == 0 {
		s.push_str("\x1B[2m");
	}
	s.push_str(&e.name);
	s.push_str("\x1B[m");
	s
}

fn format_size(cmd: &List, e: &Entry) -> String {
	let mut s = String::new();
	if cmd.compressed_size && e.decompressed_size.is_some() {
		s.push_str(&format_size2(cmd, e.compressed_size));
	}
	match e.compression_mode {
		Some(CompressMode::Mode1) => s.push('⇒'),
		Some(CompressMode::Mode2) => s.push('→'),
		None if e.decompressed_size.is_some() => s.push('⇢'),
		None => {},
	}
	s.push_str(&format_size2(cmd, e.decompressed_size.unwrap_or(e.compressed_size)));
	s
}

fn format_size2(cmd: &List, size: usize) -> String {
	if cmd.bytes {
		size.to_string()
	} else {
		use number_prefix::NumberPrefix as NP;
		let n = if cmd.binary {
			NP::binary(size as f64)
		} else {
			NP::decimal(size as f64)
		};
		match n {
			NP::Standalone(n) => n.round().to_string(),
			NP::Prefixed(p, n) => format!("{}{}", n.round(), p.symbol()),
		}
	}
}

fn get_color(ext: &str) -> Option<u8> {
	Some(match ext {
		"_x2" =>  2, // model, green
		"_x3" =>  2, // model, green
		"_hd" => 10, // shadow mesh, bright green
		"_ct" => 10, // collision mesh, bright green

		"_sn" =>  3, // scena, yellow
		"_dt" => 11, // data or aniscript, bright yellow

		"_ef" =>  4, // effect, blue
		"_ep" => 12, // effect placement, bright blue

		"_ds" =>  5, // texture, purple
		"_ch" =>  5, // image, purple
		"_cp" => 13, // sprite, bright purple

		"wav" =>  6, // sound, cyan

		_ => return None
	})
}

fn get_entries(cmd: &List, dir_file: &Path) -> eyre::Result<Vec<Entry>> {
	let mut globset = globset::GlobSetBuilder::new();
	for glob in &cmd.glob {
		let glob = globset::GlobBuilder::new(glob)
			.case_insensitive(true)
			.backslash_escape(true)
			.empty_alternates(true)
			.literal_separator(false)
			.build()?;
		globset.add(glob);
	}
	let globset = globset.build()?;

	let mut entries = dirdat::read_dir(&mmap(dir_file)?)?
		.into_iter()
		.filter(|e| globset.is_empty() || globset.is_match(&e.name))
		.filter(|e| cmd.all || e.timestamp != 0)
		.enumerate()
		.map(|(index, dirent)| Entry {
			dirent,
			index: index as u16,
			decompressed_size: None,
			compression_mode: None,
		})
		.collect::<Vec<_>>();

	if !cmd.compressed && (cmd.size || cmd.long || cmd.sort == SortColumn::Size) {
		if let Ok(dat) = mmap(&dir_file.with_extension("dat")) {
			for m in &mut entries {
				let info = m.range()
					.and_then(|r| dat.get(r))
					.and_then(bzip::compression_info_ed6);
				if let Some(info) = info {
					m.decompressed_size = Some(info.0);
					m.compression_mode = info.1;
				}
			}
		}
	}

	match cmd.sort {
		SortColumn::Id => {},
		SortColumn::Name => entries.sort_by(|a, b| a.name.cmp(&b.name)),
		SortColumn::Size => entries.sort_by_key(|e| e.decompressed_size.unwrap_or(e.compressed_size)),
		SortColumn::CSize => entries.sort_by_key(|e| e.compressed_size),
		SortColumn::Time => entries.sort_by_key(|e| e.timestamp),
	}

	if cmd.reverse {
		entries.reverse();
	}

	Ok(entries)
}

fn get_archive_number(path: &Path) -> Option<u8> {
	let name = path
		.file_name()?
		.to_str()?
		.strip_prefix("ED6_DT")?
		.strip_suffix(".dir")?;
	u8::from_str_radix(name, 16).ok()
}

fn print_grid(cmd: &List, group: usize, cells: Vec<Cell>) {
	let width = if cmd.oneline {
		0
	} else if let Some(dim) = term_size::dimensions_stdout() {
		dim.0
	} else {
		0
	};

	let orientation = if cmd.across { Orientation::Horizontal } else { Orientation::Vertical };
	print!("{}", Grid::best_fit(width, orientation, group, &cells, " "));
}