use camino::{Utf8PathBuf, Utf8Path};
use clap::builder::TypedValueParser;

#[tracing::instrument(fields(path=%path))]
pub fn mmap(path: &Utf8Path) -> eyre::Result<memmap2::Mmap> {
	let file = std::fs::File::open(path)?;
	Ok(unsafe { memmap2::Mmap::map(&file)? })
}

#[tracing::instrument(fields(path=%path))]
pub fn mmap_mut(path: &Utf8Path) -> eyre::Result<memmap2::MmapMut> {
	let file = std::fs::File::options().read(true).write(true).open(path)?;
	Ok(unsafe { memmap2::MmapMut::map_mut(&file)? })
}

pub fn glob_parser() -> impl clap::builder::TypedValueParser<Value=globset::Glob> {
	clap::builder::StringValueParser::new().try_map(|glob| {
		globset::GlobBuilder::new(&glob)
			.case_insensitive(true)
			.backslash_escape(true)
			.empty_alternates(true)
			.literal_separator(false)
			.build()
	})
}

pub fn output(output: Option<&Utf8Path>, file: &Utf8Path, extension: &str, n_inputs: usize) -> eyre::Result<Utf8PathBuf> {
	let dir = if let Some(output) = output.as_ref() {
		if n_inputs == 1 && !output.as_str().ends_with(std::path::is_separator) {
			if let Some(parent) = output.parent() {
				std::fs::create_dir_all(parent)?;
			}
			return Ok(output.to_path_buf())
		}

		std::fs::create_dir_all(output)?;
		output
	} else {
		file.parent().ok_or_else(|| eyre::eyre!("file has no parent"))?
	};
	let name = file.file_name().ok_or_else(|| eyre::eyre!("file has no name"))?;
	Ok(dir.join(name).with_extension(extension))
}
