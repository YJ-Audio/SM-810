use crate::{Error, Store};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Point {
	pub id: i64,
	pub x: f32,
	pub y: f32,
}
fn random(id: i64, salt: u64) -> f32 {
	let mut value = (id as u64).wrapping_add(salt).wrapping_add(0x9e3779b97f4a7c15);
	value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
	value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
	((value ^ (value >> 31)) >> 40) as f32 / (1u32 << 24) as f32
}
pub fn seed(id: i64) -> Point {
	Point {
		id,
		x: 0.1 + random(id, 0) * 0.8,
		y: 0.1 + random(id, 810) * 0.8,
	}
}
pub fn place(store: &Store, id: i64, existing: &[Point]) -> Result<Point, Error> {
	if existing.len() < 8 {
		return Ok(seed(id));
	}
	let candidates: HashSet<_> = existing.iter().map(|p| p.id).collect();
	let query = store.vector(id).ok_or(Error::Invalid)?;
	let hits = store.search(query, 8, Some(&candidates), Some(id))?;
	let positions: HashMap<_, _> = existing.iter().map(|p| (p.id, *p)).collect();
	let (mut x, mut y, mut weight) = (0.0, 0.0, 0.0);
	for hit in hits {
		if let Some(point) = positions.get(&hit.id) {
			let w = 1.0 / (1.0 - hit.score).max(0.001);
			x += point.x * w;
			y += point.y * w;
			weight += w;
		}
	}
	if weight == 0.0 {
		return Ok(seed(id));
	}
	Ok(Point {
		id,
		x: (x / weight + (random(id, 17) - 0.5) * 0.008).clamp(0.0, 1.0),
		y: (y / weight + (random(id, 42) - 0.5) * 0.008).clamp(0.0, 1.0),
	})
}

pub fn normalize(points: &mut [Point]) -> Result<(), Error> {
	if points.iter().any(|p| !p.x.is_finite() || !p.y.is_finite()) {
		return Err(Error::Invalid);
	}
	if points.is_empty() {
		return Ok(());
	}
	let min_x = points.iter().map(|p| p.x).fold(f32::INFINITY, f32::min);
	let max_x = points.iter().map(|p| p.x).fold(f32::NEG_INFINITY, f32::max);
	let min_y = points.iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
	let max_y = points.iter().map(|p| p.y).fold(f32::NEG_INFINITY, f32::max);
	let scale = (max_x - min_x).max(max_y - min_y).max(1e-6);
	let center = [(min_x + max_x) / 2.0, (min_y + max_y) / 2.0];
	for point in points {
		point.x = (point.x - center[0]) / scale * 0.9 + 0.5;
		point.y = (point.y - center[1]) / scale * 0.9 + 0.5;
	}
	Ok(())
}

// Fit either a rotation or a reflection; UMAP's arbitrary handedness must not flip remembered positions.
pub fn align(points: &mut [Point], previous: &[Point]) -> Result<(), Error> {
	if points
		.iter()
		.chain(previous)
		.any(|p| !p.x.is_finite() || !p.y.is_finite())
	{
		return Err(Error::Invalid);
	}
	let old: HashMap<_, _> = previous.iter().map(|p| (p.id, *p)).collect();
	let pairs: Vec<_> = points.iter().filter_map(|p| old.get(&p.id).map(|q| (*p, *q))).collect();
	if pairs.len() < 2 {
		return normalize(points);
	}
	let count = pairs.len() as f64;
	let mut a = [0.0; 2];
	let mut b = [0.0; 2];
	for (p, q) in &pairs {
		a[0] += p.x as f64 / count;
		a[1] += p.y as f64 / count;
		b[0] += q.x as f64 / count;
		b[1] += q.y as f64 / count;
	}
	let mut c = [0.0; 4];
	let mut norm = 0.0;
	for (p, q) in &pairs {
		let x = p.x as f64 - a[0];
		let y = p.y as f64 - a[1];
		let u = q.x as f64 - b[0];
		let v = q.y as f64 - b[1];
		c[0] += x * u;
		c[1] += x * v;
		c[2] += y * u;
		c[3] += y * v;
		norm += x * x + y * y;
	}
	if norm < 1e-12 {
		return normalize(points);
	}
	let rotation = [c[0] + c[3], c[1] - c[2]];
	let reflection = [c[0] - c[3], c[1] + c[2]];
	let r = rotation[0].hypot(rotation[1]);
	let f = reflection[0].hypot(reflection[1]);
	let (v, length, sign) = if r >= f {
		(rotation, r, 1.0)
	} else {
		(reflection, f, -1.0)
	};
	if length < 1e-12 {
		return normalize(points);
	}
	let cosine = v[0] / length;
	let sine = v[1] / length;
	let scale = length / norm;
	for point in points.iter_mut() {
		let x = point.x as f64 - a[0];
		let y = point.y as f64 - a[1];
		point.x = (b[0] + scale * (cosine * x - sign * sine * y)) as f32;
		point.y = (b[1] + scale * (sine * x + sign * cosine * y)) as f32;
	}
	// A uniform shrink only when new points exceed the canvas preserves orientation and aspect ratio.
	let extent = points
		.iter()
		.map(|p| (p.x - 0.5).abs().max((p.y - 0.5).abs()))
		.fold(0.5f32, f32::max);
	if extent > 0.5 {
		for point in points {
			point.x = (point.x - 0.5) / extent * 0.5 + 0.5;
			point.y = (point.y - 0.5) / extent * 0.5 + 0.5;
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn procrustes_removes_rotation_reflection_scale_translation_with_partial_overlap() {
		let previous = vec![
			Point { id: 1, x: 0.1, y: 0.2 },
			Point { id: 2, x: 0.8, y: 0.3 },
			Point { id: 3, x: 0.4, y: 0.9 },
			Point { id: 4, x: 0.7, y: 0.7 },
		];
		for flip in [-1.0, 1.0] {
			let mut points: Vec<_> = previous
				.iter()
				.map(|p| Point {
					id: p.id,
					x: 2.0 + flip * p.y * 3.0,
					y: 4.0 - p.x * 3.0,
				})
				.collect();
			let reference = previous[..3].to_vec();
			align(&mut points, &reference).unwrap();
			for (a, b) in points.iter().zip(&previous) {
				assert!((a.x - b.x).abs() < 1e-5);
				assert!((a.y - b.y).abs() < 1e-5);
			}
		}
	}
	#[test]
	fn new_points_follow_eight_neighbors_without_moving_old_points() {
		let mut store = Store::new(3);
		let mut previous = Vec::new();
		for id in 1..=8 {
			store.insert(id, vec![1.0, 0.0, 0.0]).unwrap();
			previous.push(Point { id, x: 0.2, y: 0.3 });
		}
		for id in 9..=16 {
			store.insert(id, vec![0.0, 1.0, 0.0]).unwrap();
			previous.push(Point { id, x: 0.8, y: 0.9 });
		}
		store.insert(17, vec![1.0, 0.0, 0.0]).unwrap();
		let copy = previous.clone();
		let point = place(&store, 17, &previous).unwrap();
		assert!((point.x - 0.2).abs() < 0.005 && (point.y - 0.3).abs() < 0.005);
		assert_eq!(point, place(&store, 17, &previous).unwrap());
		assert_eq!(previous, copy);
	}
}
