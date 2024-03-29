use std::fs::File;
use std::io::{prelude::*, SeekFrom};
use std::time::SystemTime;

use camino::{Utf8PathBuf, Utf8Path};
use clap::ValueHint;
use clap::builder::TypedValueParser;

use eyre_span::emit;
use falcompress::bzip;
use crate::dirdat::{self, DirEntry, Name};

#[derive(Debug, Clone, clap::Args)]
#[command(arg_required_else_help = true)]
/// Adds or replaces one or more files into an archive file.
///
/// If the file to be added already exists in the archive, it will be updated.
/// This may require expanding the dat file, leaving a gap where the previous data was.
/// To eliminate this gap, use `factorial rebuild`.
pub struct Command {
	/// Compress newly-added files (updated files keep existing compression)
	#[clap(
		short='c', long,
		require_equals = true, num_args=0..=1, default_missing_value="2",
		value_parser = clap::builder::PossibleValuesParser::new(["1", "2"])
			.map(|a| match a.as_str() {
				"1" => bzip::CompressMode::Mode1,
				"2" => bzip::CompressMode::Mode2,
				_ => unreachable!()
			})
	)]
	compression: Option<bzip::CompressMode>,

	/// Reserve space in the data for later updates
	#[clap(short, long)]
	reserve: Option<usize>,

	/// .dir file to insert into
	#[clap(value_hint = ValueHint::FilePath, required = true)]
	dir_file: Utf8PathBuf,

	/// Files to insert
	#[clap(value_hint = ValueHint::FilePath, required = true)]
	file: Vec<Utf8PathBuf>,
}

#[tracing::instrument(skip_all, fields(path=%cmd.dir_file))]
pub fn run(cmd: &Command) -> eyre::Result<()> {
	let mut dir = dirdat::read_dir(&std::fs::read(&cmd.dir_file)?)?;

	let mut dat = File::options()
		.read(true)
		.write(true)
		.truncate(false)
		.append(false)
		.create(false)
		.open(&cmd.dir_file.with_extension("dat"))?;

	dat.seek(SeekFrom::Start(0))?;
	eyre::ensure!(dat.read_array()? == *b"LB DAT\x1A\0", "invalid dat file");

	for file in &cmd.file {
		emit(add(cmd, &mut dir, &mut dat, file));
	}

	std::fs::write(&cmd.dir_file, dirdat::write_dir(&dir))?;

	Ok(())
}

#[tracing::instrument(skip_all, fields(file=%file))]
fn add(cmd: &Command, dir: &mut [DirEntry], dat: &mut File, file: &Utf8Path) -> eyre::Result<()> {
	// Starting with a stat call gives us a nice error if it doesn't exist
	let timestamp = std::fs::metadata(file)?
		.modified()
		.unwrap_or_else(|_| SystemTime::now());
	let timestamp = timestamp.duration_since(SystemTime::UNIX_EPOCH)?.as_secs();

	let name = Name::try_from(file.file_name().unwrap())?;

	let id = get_id(dir, name)?;
	let ent = &mut dir[id];

	let exists = ent.timestamp != 0;

	let compression = if exists {
		dat.seek(SeekFrom::Start(16 + 4 * id as u64))?;
		let dat_offset = u32::from_le_bytes(dat.read_array()?) as usize;
		eyre::ensure!(dat_offset == ent.offset, "mismatched dat file offset");
		dat.seek(SeekFrom::Start(ent.offset as u64))?;
		let mut existing = vec![0; ent.size];
		dat.read_exact(&mut existing)?;
		bzip::compression_info_ed6(&existing).map(|a| a.1.unwrap_or_default())
	} else {
		cmd.compression
	};

	match compression {
		Some(bzip::CompressMode::Mode1) => tracing::debug!("using compression mode 1"),
		Some(bzip::CompressMode::Mode2) => tracing::debug!("using compression mode 2"),
		None => tracing::debug!("using no compression"),
	}

	let data = std::fs::read(file)?;
	let mut data = match compression {
		Some(method) => bzip::compress_ed6_to_vec(&data, method),
		None => data,
	};
	let size = data.len();

	while data.len() < cmd.reserve.unwrap_or(0) {
		data.push(0);
	}

	let needs_alloc = if !exists {
		tracing::debug!("allocating");
		true
	} else if data.len() > ent.reserved_size {
		tracing::warn!("reallocating");
		true
	} else {
		tracing::debug!("reusing allocation");
		false
	};

	if needs_alloc {
		let pos = dat.seek(SeekFrom::End(0))?;
		dat.write_all(&data)?;
		dat.seek(SeekFrom::Start(16 + 4 * id as u64))?;
		dat.write_all(&u32::to_le_bytes(pos as u32))?;
		if exists {
			dat.seek(SeekFrom::Start(ent.offset as u64))?;
			dat.write_all(&vec![0; ent.reserved_size.max(ent.size)])?;
		}
		ent.offset = pos as usize;
		ent.reserved_size = cmd.reserve.unwrap_or(data.len());
	} else {
		dat.seek(SeekFrom::Start(ent.offset as u64))?;
		dat.write_all(&data)?;
	}

	ent.size = size;
	ent.timestamp = timestamp as u32;

	tracing::info!("added {} as {:04X}", ent.name, id);

	Ok(())
}

#[tracing::instrument(skip_all)]
fn get_id(dir: &mut [DirEntry], name: Name) -> eyre::Result<usize> {
	if let Some(id) = dir.iter().position(|e| e.name == name) {
		tracing::debug!("found existing at {id:04X}");
		Ok(id)
	} else if let Some(id) = dir.iter().position(|e| e.name == Name::default()) {
		dir[id].name = name;
		tracing::debug!("found empty at {id:04X}");
		Ok(id)
	} else {
		eyre::bail!("no more space in index; use `factoria rebuild` to allocate more");
	}
}

trait ReadArray: Read {
	fn read_array<const N: usize>(&mut self) -> std::io::Result<[u8; N]> {
		let mut buf = [0; N];
		self.read_exact(&mut buf)?;
		Ok(buf)
	}
}
impl<T: Read> ReadArray for T {}
