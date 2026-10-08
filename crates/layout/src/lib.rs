use ndarray::{Array2, ArrayView2};
use rayon::prelude::*;
use sampler_similarity::{
	Store,
	layout::{Point, normalize, seed},
};
use std::{
	collections::HashMap,
	fs::File,
	io::{Read, Write},
	path::Path,
};
use thiserror::Error;
#[derive(Debug, Error)]
pub enum Error {
	#[error("layout I/O: {0}")]
	Io(#[from] std::io::Error),
	#[error("layout vectors: {0}")]
	Vector(#[from] sampler_similarity::Error),
	#[error("layout: {0}")]
	Invalid(String),
}
pub fn write_input(path: &Path, ids: &[i64], dimensions: usize, vectors: &[f32]) -> Result<(), Error> {
	if dimensions == 0 || dimensions > 4096 || ids.len() > 100000 || vectors.len() != ids.len() * dimensions {
		return Err(Error::Invalid("Invalid matrix shape".into()));
	}
	let mut file = std::io::BufWriter::new(File::create(path)?);
	file.write_all(b"SMLYT001")?;
	file.write_all(&(ids.len() as u32).to_le_bytes())?;
	file.write_all(&(dimensions as u32).to_le_bytes())?;
	for (&id, row) in ids.iter().zip(vectors.chunks_exact(dimensions)) {
		file.write_all(&id.to_le_bytes())?;
		for value in row {
			file.write_all(&value.to_le_bytes())?;
		}
	}
	file.flush()?;
	Ok(())
}
pub fn read_input(path: &Path) -> Result<(Store, usize), Error> {
	let mut file = std::io::BufReader::new(File::open(path)?);
	let mut header = [0u8; 16];
	file.read_exact(&mut header)?;
	if &header[..8] != b"SMLYT001" {
		return Err(Error::Invalid("Invalid input header".into()));
	}
	let count = u32::from_le_bytes(header[8..12].try_into().expect("header count")) as usize;
	let dimensions = u32::from_le_bytes(header[12..16].try_into().expect("header dimensions")) as usize;
	if count > 100000 || dimensions == 0 || dimensions > 4096 {
		return Err(Error::Invalid("Input exceeds shape limits".into()));
	}
	let expected = 16 + count as u64 * (8 + dimensions as u64 * 4);
	if file.get_ref().metadata()?.len() != expected {
		return Err(Error::Invalid("Input length does not match header".into()));
	}
	let mut ids = Vec::with_capacity(count);
	let mut vectors = Vec::with_capacity(count * dimensions);
	let mut word = [0u8; 8];
	for _ in 0..count {
		file.read_exact(&mut word)?;
		ids.push(i64::from_le_bytes(word));
		for _ in 0..dimensions {
			file.read_exact(&mut word[..4])?;
			vectors.push(f32::from_le_bytes(word[..4].try_into().expect("f32 word")));
		}
	}
	Ok((Store::from_vectors(ids, vectors, dimensions)?, dimensions))
}
pub fn write_output(path: &Path, points: &[Point]) -> Result<(), Error> {
	let mut file = std::io::BufWriter::new(File::create(path)?);
	file.write_all(b"SMXY0001")?;
	file.write_all(&(points.len() as u32).to_le_bytes())?;
	file.write_all(&[0; 4])?;
	for point in points {
		file.write_all(&point.id.to_le_bytes())?;
		file.write_all(&point.x.to_le_bytes())?;
		file.write_all(&point.y.to_le_bytes())?;
	}
	file.flush()?;
	Ok(())
}
pub fn read_output(path: &Path) -> Result<Vec<Point>, Error> {
	let bytes = std::fs::read(path)?;
	if bytes.len() < 16 || &bytes[..8] != b"SMXY0001" {
		return Err(Error::Invalid("Invalid output header".into()));
	}
	let count = u32::from_le_bytes(bytes[8..12].try_into().expect("header count")) as usize;
	if count > 100000 || bytes.len() != 16 + count * 16 {
		return Err(Error::Invalid("Invalid output length".into()));
	}
	let mut points = Vec::with_capacity(count);
	for record in bytes[16..].as_chunks::<16>().0 {
		let point = Point {
			id: i64::from_le_bytes(record[..8].try_into().expect("record id")),
			x: f32::from_le_bytes(record[8..12].try_into().expect("record x")),
			y: f32::from_le_bytes(record[12..16].try_into().expect("record y")),
		};
		if !point.x.is_finite() || !point.y.is_finite() {
			return Err(Error::Invalid("Nonfinite projection".into()));
		}
		points.push(point);
	}
	Ok(points)
}
pub fn project(store: &Store, dimensions: usize) -> Result<Vec<Point>, Error> {
	let n = store.len();
	if n < 4 {
		let mut points: Vec<_> = store.ids().iter().map(|&id| seed(id)).collect();
		normalize(&mut points)?;
		return Ok(points);
	}
	let k = 15.min(n - 1);
	let index: HashMap<_, _> = store
		.ids()
		.iter()
		.enumerate()
		.map(|(index, &id)| (id, index as u32))
		.collect();
	let neighbors: Result<Vec<_>, Error> = store
		.ids()
		.par_iter()
		.map(|&id| {
			let hits = store.search(store.vector(id).expect("store row exists"), k - 1, None, Some(id))?;
			let mut indices = vec![index[&id]];
			let mut distances = vec![0.0];
			for hit in hits {
				indices.push(index[&hit.id]);
				distances.push((2.0 * (1.0 - hit.score)).max(0.0).sqrt());
			}
			Ok((indices, distances))
		})
		.collect();
	let (indices, distances): (Vec<_>, Vec<_>) = neighbors?.into_iter().unzip();
	let indices = Array2::from_shape_vec((n, k), indices.concat()).map_err(|e| Error::Invalid(e.to_string()))?;
	let distances = Array2::from_shape_vec((n, k), distances.concat()).map_err(|e| Error::Invalid(e.to_string()))?;
	let initial = Array2::from_shape_fn((n, 2), |(row, column)| {
		let p = seed(store.ids()[row]);
		((if column == 0 { p.x } else { p.y }) - 0.5) * 20.0
	});
	let data = ArrayView2::from_shape((n, dimensions), store.vectors()).map_err(|e| Error::Invalid(e.to_string()))?;
	let config = umap_rs::UmapConfig {
		n_components: 2,
		graph: umap_rs::GraphParams {
			n_neighbors: k,
			..Default::default()
		},
		optimization: umap_rs::OptimizationParams {
			n_epochs: Some(500),
			..Default::default()
		},
		..Default::default()
	};
	let umap = umap_rs::Umap::new(config);
	let coordinates = if std::env::var_os("SAMPLER_LAYOUT_UPSTREAM").is_some() {
		umap.fit(data, indices.view(), distances.view(), initial.view())
			.into_embedding()
	} else {
		let manifold = umap.learn_manifold(data, indices.view(), distances.view());
		optimize(manifold.graph(), initial)
	};
	let mut points: Vec<_> = store
		.ids()
		.iter()
		.enumerate()
		.map(|(row, &id)| Point {
			id,
			x: coordinates[[row, 0]],
			y: coordinates[[row, 1]],
		})
		.collect();
	normalize(&mut points)?;
	Ok(points)
}
// The 0.4.5 upstream optimizer adds an extra squared-distance factor to both gradient denominators.
// Keep graph construction in the crate and use the derivative of 1/(1+a*r^(2b)) here.
fn optimize(graph: &umap_rs::SparseMat, mut xy: Array2<f32>) -> Array2<f32> {
	let n = xy.nrows();
	for dimension in 0..2 {
		let low = xy.column(dimension).iter().copied().fold(f32::INFINITY, f32::min);
		let high = xy.column(dimension).iter().copied().fold(f32::NEG_INFINITY, f32::max);
		for row in 0..n {
			xy[[row, dimension]] = (xy[[row, dimension]] - low) / (high - low).max(1e-6) * 10.0;
		}
	}
	let maximum = graph.data().iter().copied().fold(0.0f32, f32::max);
	let mut edges = Vec::new();
	for (row, values) in graph.outer_iterator().enumerate() {
		for (&column, &weight) in values.indices().iter().zip(values.data()) {
			let column = column as usize;
			if row != column && weight >= maximum / 500.0 {
				let period = maximum as f64 / weight as f64;
				edges.push((row, column, period, period, period / 5.0));
			}
		}
	}
	let mut random = 810u64;
	let next = |state: &mut u64| {
		*state ^= *state << 13;
		*state ^= *state >> 7;
		*state ^= *state << 17;
		*state
	};
	for i in (1..edges.len()).rev() {
		let j = next(&mut random) as usize % (i + 1);
		edges.swap(i, j);
	}
	let a = 1.5769435f32;
	let b = 0.8950609f32;
	for epoch in 0..500 {
		let alpha = 1.0 - epoch as f32 / 500.0;
		for (head, tail, period, next_positive, next_negative) in &mut edges {
			if *next_positive > epoch as f64 {
				continue;
			}
			let delta = [xy[[*head, 0]] - xy[[*tail, 0]], xy[[*head, 1]] - xy[[*tail, 1]]];
			let square = delta[0] * delta[0] + delta[1] * delta[1];
			if square > 0.0 {
				let power = square.powf(b);
				let coefficient = -2.0 * a * b * power / (square * (1.0 + a * power));
				for d in 0..2 {
					let change = (coefficient * delta[d]).clamp(-4.0, 4.0) * alpha;
					xy[[*head, d]] += change;
					xy[[*tail, d]] -= change;
				}
			}
			*next_positive += *period;
			let negative_period = *period / 5.0;
			let negatives = ((epoch as f64 - *next_negative) / negative_period).max(0.0) as usize;
			for _ in 0..negatives {
				let other = next(&mut random) as usize % n;
				if other == *head {
					continue;
				}
				let delta = [xy[[*head, 0]] - xy[[other, 0]], xy[[*head, 1]] - xy[[other, 1]]];
				let square = delta[0] * delta[0] + delta[1] * delta[1];
				if square > 0.0 {
					let coefficient = 2.0 * b / ((0.001 + square) * (1.0 + a * square.powf(b)));
					for d in 0..2 {
						xy[[*head, d]] += (coefficient * delta[d]).clamp(-4.0, 4.0) * alpha;
					}
				}
			}
			*next_negative += negatives as f64 * negative_period;
		}
	}
	xy
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn projection_preserves_separated_neighborhoods_and_is_repeatable() {
		let ids: Vec<_> = (1..=90).collect();
		let vectors: Vec<f32> = (0..90)
			.flat_map(|row| {
				(0..8).map(move |column| {
					let cluster = row / 30;
					let jitter = seed((row * 8 + column) as i64);
					if column == cluster {
						1.0
					} else {
						(jitter.x - 0.5) * 0.15
					}
				})
			})
			.collect();
		let store = Store::from_vectors(ids, vectors, 8).unwrap();
		let points = project(&store, 8).unwrap();
		let second = project(&store, 8).unwrap();
		let mut same_cluster = 0;
		for (index, point) in points.iter().enumerate() {
			assert!((0.0..=1.0).contains(&point.x) && (0.0..=1.0).contains(&point.y));
			assert!((point.x - second[index].x).abs() < 1e-5);
			assert!((point.y - second[index].y).abs() < 1e-5);
			let mut neighbors: Vec<_> = points
				.iter()
				.enumerate()
				.filter(|(other, _)| *other != index)
				.map(|(other, p)| (other, (p.x - point.x).powi(2) + (p.y - point.y).powi(2)))
				.collect();
			neighbors.sort_by(|a, b| a.1.total_cmp(&b.1));
			same_cluster += neighbors
				.iter()
				.take(5)
				.filter(|(other, _)| other / 30 == index / 30)
				.count();
		}
		assert!(
			same_cluster as f32 / 450.0 > 0.95,
			"Separated source neighborhoods must survive projection"
		);
	}
	#[test]
	fn binary_protocol_round_trips_and_rejects_invalid_output() {
		let dir = tempfile::tempdir().unwrap();
		let input = dir.path().join("input");
		let output = dir.path().join("output");
		write_input(&input, &[42, 99], 3, &[2.0, 0.0, 0.0, 0.0, 3.0, 0.0]).unwrap();
		let (store, dimensions) = read_input(&input).unwrap();
		assert_eq!(dimensions, 3);
		assert_eq!(store.ids(), &[42, 99]);
		assert_eq!(store.vector(42).unwrap(), &[1.0, 0.0, 0.0]);
		let points = project(&store, dimensions).unwrap();
		write_output(&output, &points).unwrap();
		assert_eq!(read_output(&output).unwrap(), points);
		let mut bytes = std::fs::read(&output).unwrap();
		bytes.truncate(20);
		std::fs::write(&output, &bytes).unwrap();
		assert!(read_output(&output).is_err());
	}
}
