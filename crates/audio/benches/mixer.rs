use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use sampler_audio::{Buffer, Command, Mixer, StreamBuffer};
use std::{hint::black_box, sync::Arc};
fn mixer(c: &mut Criterion) {
	let buffer = Arc::new(Buffer {
		samples: vec![0.1; 48000 * 2].into_boxed_slice(),
		channels: 2,
	});
	c.bench_function("stereo_256_frames", |b| {
		b.iter_batched_ref(
			|| {
				let (mut mixer, mut commands, garbage, position) = Mixer::new(48000, 2);
				commands
					.push(Command::Play {
						id: 1,
						buffer: buffer.clone(),
						gain: 1.0,
						onset: None,
					})
					.ok()
					.unwrap();
				let mut output = [0.0f32; 512];
				mixer.render(&mut output);
				(mixer, output, commands, garbage, position)
			},
			|(mixer, output, _, _, _)| mixer.render(black_box(output)),
			BatchSize::SmallInput,
		)
	});
	c.bench_function("streamed_stereo_256_frames", |b| {
		b.iter_batched_ref(
			|| {
				let (mut mixer, mut commands, garbage, position) = Mixer::new(48000, 2);
				let (mut writer, stream) = StreamBuffer::bounded(2, 1024).unwrap();
				writer.write(&[0.1; 2048]);
				commands
					.push(Command::Stream {
						id: 1,
						stream,
						gain: 1.0,
						onset: None,
					})
					.ok()
					.unwrap();
				let mut output = [0.0f32; 512];
				mixer.render(&mut output);
				(mixer, output, commands, garbage, position, writer)
			},
			|(mixer, output, _, _, _, _)| mixer.render(black_box(output)),
			BatchSize::SmallInput,
		)
	});
}
criterion_group!(benches, mixer);
criterion_main!(benches);
