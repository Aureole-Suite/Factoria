use std::collections::BTreeMap;
use std::io::{prelude::*, SeekFrom};
use std::time::SystemTime;

use camino::{Utf8PathBuf, Utf8Path};
use clap::ValueHint;
use indicatif::ProgressIterator;
use serde::de::{self, Deserialize};
use eyre_span::emit;

use falcompress::bzip;
use crate::dirdat::{self, DirEntry, Name};

#[derive(Debug, Clone, clap::Args)]
#[command(arg_required_else_help = true)]
pub struct Command {
	/// Location of the resulting .dir file. .dat is placed next to it.
	#[clap(long, short, value_hint = ValueHint::DirPath)]
	output: Option<Utf8PathBuf>,

	/// The .json indexes to reconstruct
	#[clap(value_hint = ValueHint::FilePath, required = true)]
	json_file: Vec<Utf8PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct FileId(u16);

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(remote = "Entry")]
struct Entry {
	path: Option<Utf8PathBuf>,
	name: Option<String>,
	#[serde(default, deserialize_with="parse_compress_mode")]
	compress: Option<bzip::CompressMode>,
	reserve: Option<usize>,
	#[serde(default)]
	unknown1: u32,
	#[serde(default)]
	unknown2: usize,
}

pub fn run(cmd: &Command) -> eyre::Result<()> {
	for json_file in &cmd.json_file {
		emit(create(cmd, json_file));
	}
	Ok(())
}

#[tracing::instrument(skip_all, fields(path=%json_file, out))]
fn create(cmd: &Command, json_file: &Utf8Path) -> eyre::Result<()> {
	let json: BTreeMap<FileId, Option<Entry>>
		= serde_json::from_reader(std::fs::File::open(json_file)?)?;

	let out_dir = crate::util::output(cmd.output.as_deref(), json_file, "dir", cmd.json_file.len())?;

	tracing::Span::current().record("out", tracing::field::display(&out_dir));
	std::fs::create_dir_all(out_dir.parent().unwrap())?;

	let size = json.last_key_value().map(|a| a.0.0 + 1).unwrap_or_default() as usize;
	let mut entries = vec![None; size];
	for (k, v) in json {
		entries[k.0 as usize] = v
	}

	// TODO lots of duplicated code between here and rebuild

	let mut out_dat = std::fs::File::create(out_dir.with_extension("dat.tmp"))?;
	out_dat.write_all(b"LB DAT\x1A\0")?;
	out_dat.write_all(&u64::to_le_bytes(size as u64))?;
	for _ in 0..=size {
		out_dat.write_all(&u32::to_le_bytes(0))?;
	}

	let mut dir = Vec::with_capacity(size);
	let style = indicatif::ProgressStyle::with_template("{bar} {prefix} {pos}/{len}").unwrap()
		.progress_chars("█🮆🮅🮄▀🮃🮂▔ ");
	let ind = indicatif::ProgressBar::new(entries.iter().filter(|a| a.is_some()).count() as _)
		.with_style(style)
		.with_prefix(out_dir.to_string());
	let iter = par_map(
		entries.into_iter(),
		{
			let json_file = json_file.to_owned();
			move |e| process_entry(e, &json_file)
		},
	).progress_with(ind.clone());
	for (id, e) in iter.enumerate() {
		let (mut ent, data) = e?;

		if let Some(data) = data {
			let pos = out_dat.seek(SeekFrom::End(0))?;
			ent.offset = pos as usize;
			out_dat.write_all(&data)?;
			let pos2 = out_dat.seek(SeekFrom::End(0))?;
			out_dat.seek(SeekFrom::Start(16 + 4 * id as u64))?;
			out_dat.write_all(&u32::to_le_bytes(pos as u32))?;
			out_dat.write_all(&u32::to_le_bytes(pos2 as u32))?;
		}
		dir.push(ent)
	}
	ind.abandon();

	std::fs::rename(out_dir.with_extension("dat.tmp"), out_dir.with_extension("dat"))?;
	std::fs::write(&out_dir, dirdat::write_dir(&dir))?;
	
	tracing::info!("created");

	Ok(())
}

fn process_entry(e: Option<Entry>, json_file: &Utf8Path) -> eyre::Result<(DirEntry, Option<Vec<u8>>)> {
	let mut ent = DirEntry::default();
	let data = if let Some(e) = e {
		let name = match &e {
			Entry { name: Some(name), .. } => name.as_str(),
			Entry { path: Some(path), .. } => path.file_name().unwrap(),
			_ => unreachable!()
		};
		let _span = tracing::info_span!("file", name=%name, path=tracing::field::Empty).entered();
		ent.name = Name::try_from(name)?;
		ent.unk1 = e.unknown1;
		ent.unk2 = e.unknown2;

		if let Some(path) = &e.path {
			let path = json_file.parent().unwrap().join(path);
			_span.record("path", tracing::field::display(&path));

			let data = std::fs::read(&path)?;
			let mut data = match e.compress {
				Some(method) => bzip::compress_ed6_to_vec(&data, method),
				None => data,
			};
			ent.size = data.len();
			ent.reserved_size = e.reserve.unwrap_or(data.len());

			while data.len() < e.reserve.unwrap_or(0) {
				data.push(0);
			}

			let timestamp = std::fs::metadata(path)?
				.modified()
				.unwrap_or_else(|_| SystemTime::now());
			ent.timestamp = timestamp.duration_since(SystemTime::UNIX_EPOCH)?.as_secs() as u32;
			Some(data)
		} else {
			Some(Vec::new())
		}
	} else {
		None
	};

	Ok((ent, data))
}

fn par_map<T, U>(
	iter: impl Iterator<Item=T> + Send + 'static,
	map: impl Fn(T) -> U + Send + Sync + 'static,
) -> impl Iterator<Item=U> where
	T: Send + 'static,
	U: Send + 'static,
{
	use std::sync::mpsc;
	use rayon::prelude::*;
	let (channel_send, channel_recv) = mpsc::channel();
	std::thread::spawn(move || {
		iter.map_while(move |item| {
			let (result_send, result_recv) = mpsc::sync_channel(0);
			channel_send.send(result_recv).ok()?;
			Some((item, result_send))
		})
		.par_bridge()
		.try_for_each(|(item, result_send)| {
			result_send.send(map(item))
		})
		.ok()
	});
	channel_recv.into_iter().map_while(|a| a.recv().ok())
}

fn parse_compress_mode<'de, D: serde::Deserializer<'de>>(des: D) -> Result<Option<bzip::CompressMode>, D::Error> {
	match <Option<u8>>::deserialize(des)? {
		Some(1) => Ok(Some(bzip::CompressMode::Mode1)),
		Some(2) => Ok(Some(bzip::CompressMode::Mode2)),
		None => Ok(None),
		Some(v) => Err(de::Error::invalid_value(
			de::Unexpected::Unsigned(v as _),
			&"1, 2, or null"),
		),
	}
}

impl std::str::FromStr for Entry {
	type Err = std::convert::Infallible;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		Ok(Entry {
			path: Some(Utf8PathBuf::from(s)),
			name: None,
			compress: None,
			reserve: None,
			unknown1: 0,
			unknown2: 0,
		})
	}
}

impl<'de> Deserialize<'de> for Entry {
	fn deserialize<D: de::Deserializer<'de>>(des: D) -> Result<Self, D::Error> {
		struct V;
		impl<'de> de::Visitor<'de> for V {
			type Value = Entry;

			fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
				formatter.write_str("string or map")
			}

			fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
				std::str::FromStr::from_str(value).map_err(de::Error::custom)
			}

			fn visit_map<M: de::MapAccess<'de>>(self, map: M) -> Result<Self::Value, M::Error> {
				Entry::deserialize(de::value::MapAccessDeserializer::new(map))
			}
		}

		let v = des.deserialize_any(V)?;
		if v.path.is_none() && v.name.is_none() {
			return Err(de::Error::custom("at least one of `path` and `name` must be present"))
		}
		Ok(v)
	}
}

impl<'de> Deserialize<'de> for FileId {
	fn deserialize<D: de::Deserializer<'de>>(des: D) -> Result<Self, D::Error> {
		let s = String::deserialize(des)?;
		let err = || de::Error::invalid_value(
			de::Unexpected::Str(&s),
			&"a hexadecimal number",
		);

		let s = s.strip_prefix("0x").ok_or_else(err)?;
		let v = u32::from_str_radix(s, 16).map_err(|_| err())?;
		Ok(FileId(v as u16))
	}
}
