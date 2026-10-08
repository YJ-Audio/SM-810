pub mod layout;
use rayon::prelude::*;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use thiserror::Error;
use wide::f32x8;

#[derive(Debug, Error)]
pub enum Error {
	#[error("embedding dimensions do not match")]
	Dimensions,
	#[error("embedding is empty, non-finite, or has zero norm")]
	Invalid,
}
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Hit {
	pub id: i64,
	pub score: f32,
}

pub fn normalize(vector: &mut [f32]) -> Result<(), Error> {
	let norm = vector.iter().map(|v| (*v as f64).powi(2)).sum::<f64>().sqrt();
	if !norm.is_finite() || norm == 0.0 {
		return Err(Error::Invalid);
	}
	for value in vector {
		*value = (*value as f64 / norm) as f32;
	}
	Ok(())
}
#[inline]
fn dot(a: &[f32], b: &[f32]) -> f32 {
	let mut sum = f32x8::ZERO;
	let (left, left_tail) = a.as_chunks::<8>();
	let (right, right_tail) = b.as_chunks::<8>();
	for (a, b) in left.iter().zip(right) {
		sum += f32x8::new(*a) * f32x8::new(*b);
	}
	sum.reduce_add() + left_tail.iter().zip(right_tail).map(|(a, b)| a * b).sum::<f32>()
}
fn keep_best(best: &mut Vec<Hit>, hit: Hit, k: usize) {
	let position =
		best.partition_point(|other| other.score > hit.score || (other.score == hit.score && other.id < hit.id));
	if position < k {
		best.insert(position, hit);
		best.truncate(k);
	}
}

pub struct Store {
	dimensions: usize,
	ids: Vec<i64>,
	index: HashMap<i64, usize>,
	vectors: Vec<f32>,
}
impl Store {
	pub fn new(dimensions: usize) -> Self {
		Self {
			dimensions,
			ids: Vec::new(),
			index: HashMap::new(),
			vectors: Vec::new(),
		}
	}
	pub fn from_vectors(ids: Vec<i64>, mut vectors: Vec<f32>, dimensions: usize) -> Result<Self, Error> {
		if dimensions == 0 || vectors.len() != ids.len().saturating_mul(dimensions) {
			return Err(Error::Dimensions);
		}
		let index: HashMap<_, _> = ids.iter().enumerate().map(|(index, &id)| (id, index)).collect();
		if index.len() != ids.len() {
			return Err(Error::Invalid);
		}
		for row in vectors.chunks_exact_mut(dimensions) {
			normalize(row)?;
		}
		Ok(Self {
			dimensions,
			ids,
			index,
			vectors,
		})
	}
	pub fn vectors(&self) -> &[f32] {
		&self.vectors
	}
	pub fn ids(&self) -> &[i64] {
		&self.ids
	}
	pub fn len(&self) -> usize {
		self.ids.len()
	}
	pub fn is_empty(&self) -> bool {
		self.ids.is_empty()
	}
	pub fn insert(&mut self, id: i64, mut vector: Vec<f32>) -> Result<(), Error> {
		if vector.len() != self.dimensions || self.dimensions == 0 {
			return Err(Error::Dimensions);
		}
		normalize(&mut vector)?;
		if let Some(&index) = self.index.get(&id) {
			self.vectors[index * self.dimensions..(index + 1) * self.dimensions].copy_from_slice(&vector);
		} else {
			self.index.insert(id, self.ids.len());
			self.ids.push(id);
			self.vectors.extend(vector);
		}
		Ok(())
	}
	pub fn vector(&self, id: i64) -> Option<&[f32]> {
		let index = *self.index.get(&id)?;
		Some(&self.vectors[index * self.dimensions..(index + 1) * self.dimensions])
	}
	pub fn search(
		&self,
		query: &[f32],
		k: usize,
		candidates: Option<&HashSet<i64>>,
		exclude: Option<i64>,
	) -> Result<Vec<Hit>, Error> {
		if query.len() != self.dimensions {
			return Err(Error::Dimensions);
		}
		if self.is_empty() || k == 0 {
			return Ok(Vec::new());
		}
		let mut query = query.to_vec();
		normalize(&mut query)?;
		let k = k.min(self.len());

		if k > 128 {
			let mut hits: Vec<_> = self
				.vectors
				.par_chunks_exact(self.dimensions)
				.zip(&self.ids)
				.filter_map(|(vector, &id)| {
					if Some(id) == exclude || candidates.is_some_and(|set| !set.contains(&id)) {
						None
					} else {
						Some(Hit {
							id,
							score: dot(&query, vector).clamp(-1.0, 1.0),
						})
					}
				})
				.collect();
			hits.par_sort_unstable_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
			hits.truncate(k);
			return Ok(hits);
		}
		let scan = |start: usize, vectors: &[f32]| {
			let mut best = Vec::with_capacity(k + 1);
			for (index, vector) in vectors.chunks_exact(self.dimensions).enumerate() {
				let id = self.ids[start + index];
				if Some(id) == exclude || candidates.is_some_and(|set| !set.contains(&id)) {
					continue;
				}
				keep_best(
					&mut best,
					Hit {
						id,
						score: dot(&query, vector).clamp(-1.0, 1.0),
					},
					k,
				);
			}
			best
		};
		if self.len() < 8192 {
			return Ok(scan(0, &self.vectors));
		}
		const CHUNK: usize = 2048;
		let batches: Vec<_> = self
			.vectors
			.par_chunks(self.dimensions * CHUNK)
			.enumerate()
			.map(|(index, vectors)| scan(index * CHUNK, vectors))
			.collect();
		let mut best = Vec::with_capacity(k + 1);
		for hit in batches.into_iter().flatten() {
			keep_best(&mut best, hit, k);
		}
		Ok(best)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn normalized_cosine_filters_excludes_and_updates_without_duplicates() {
		let mut store = Store::new(3);
		store.insert(1, vec![2.0, 0.0, 0.0]).unwrap();
		store.insert(2, vec![1.0, 1.0, 0.0]).unwrap();
		store.insert(3, vec![-1.0, 0.0, 0.0]).unwrap();
		let hits = store.search(&[1.0, 0.0, 0.0], 5, None, Some(1)).unwrap();
		assert_eq!(hits.iter().map(|h| h.id).collect::<Vec<_>>(), vec![2, 3]);
		assert!((hits[0].score - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
		store.insert(3, vec![1.0, 0.0, 0.0]).unwrap();
		assert_eq!(store.len(), 3);
		let ids = HashSet::from([2, 3]);
		assert_eq!(store.search(&[1.0, 0.0, 0.0], 1, Some(&ids), None).unwrap()[0].id, 3);
		assert!(store.insert(4, vec![f32::NAN, 0.0, 0.0]).is_err());
		assert!(store.search(&[0.0, 0.0, 0.0], 1, None, None).is_err());
	}
	#[test]
	fn large_pages_agree_with_top_k_and_stable_ties() {
		let mut store = Store::new(8);
		for id in (0..300).rev() {
			store.insert(id, vec![1.0; 8]).unwrap();
		}
		let all = store.search(&[1.0; 8], 300, None, None).unwrap();
		let top = store.search(&[1.0; 8], 5, None, None).unwrap();
		assert_eq!(
			all.iter().map(|h| h.id).collect::<Vec<_>>(),
			(0..300).collect::<Vec<_>>()
		);
		assert_eq!(
			top.iter().map(|h| h.id).collect::<Vec<_>>(),
			all[..5].iter().map(|h| h.id).collect::<Vec<_>>()
		);
	}
	#[test]
	fn parallel_top_k_matches_scalar_reference() {
		let mut store = Store::new(17);
		for id in 0..10000 {
			store
				.insert(id, (0..17).map(|d| ((id * 31 + d * 13) as f32).sin()).collect())
				.unwrap();
		}
		let query = store.vector(182).unwrap();
		let hits = store.search(query, 7, None, Some(182)).unwrap();
		let mut reference = Vec::new();
		for &id in &store.ids {
			if id != 182 {
				keep_best(
					&mut reference,
					Hit {
						id,
						score: query.iter().zip(store.vector(id).unwrap()).map(|(a, b)| a * b).sum(),
					},
					7,
				);
			}
		}
		for (hit, expected) in hits.iter().zip(reference) {
			assert!((hit.score - expected.score).abs() < 1e-5);
		}
	}
}
