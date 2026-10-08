use criterion::{Criterion, criterion_group, criterion_main};
use sampler_similarity::Store;
use std::hint::black_box;
fn neighbors(c: &mut Criterion) {
	let mut store = Store::new(512);
	let mut seed = 42u64;
	for id in 0..100000 {
		let vector = (0..512)
			.map(|_| {
				seed ^= seed << 13;
				seed ^= seed >> 7;
				seed ^= seed << 17;
				(seed as u32 as f32 / u32::MAX as f32) - 0.5
			})
			.collect();
		store.insert(id, vector).unwrap();
	}
	let query = store.vector(50000).unwrap().to_vec();
	c.bench_function("100k_512d_top_5", |b| {
		b.iter(|| black_box(store.search(black_box(&query), 5, None, Some(50000)).unwrap()))
	});
}
criterion_group!(benches, neighbors);
criterion_main!(benches);
