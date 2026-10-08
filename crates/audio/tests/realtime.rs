use sampler_audio::{Buffer, Command, Mixer, StreamBuffer};
use std::{
	alloc::{GlobalAlloc, Layout, System},
	cell::Cell,
	sync::Arc,
};
thread_local! {static TRACK:Cell<bool>=const {Cell::new(false)};static ALLOCS:Cell<usize>=const {Cell::new(0)};static FREES:Cell<usize>=const {Cell::new(0)};}
struct TrackingAllocator;
unsafe impl GlobalAlloc for TrackingAllocator {
	unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
		if TRACK.try_with(Cell::get).unwrap_or(false) {
			let _ = ALLOCS.try_with(|n| n.set(n.get() + 1));
		}
		unsafe { System.alloc(layout) }
	}
	unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
		if TRACK.try_with(Cell::get).unwrap_or(false) {
			let _ = FREES.try_with(|n| n.set(n.get() + 1));
		}
		unsafe { System.dealloc(pointer, layout) }
	}
}
#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;
#[test]
fn callback_does_not_allocate_or_free_when_garbage_queue_fills() {
	let (mut mixer, mut commands, _garbage, _position) = Mixer::new(48000, 2);
	let mut block = [0f32; 512];
	for id in 1..180 {
		let buffer = Arc::new(Buffer {
			samples: vec![0.2; 4096].into_boxed_slice(),
			channels: 2,
		});
		let _ = commands.push(Command::Play { id, buffer, gain: 1.0 });
		TRACK.set(true);
		mixer.render(&mut block);
		TRACK.set(false);
	}
	assert_eq!(ALLOCS.get(), 0);
	assert_eq!(FREES.get(), 0);
}

#[test]
fn streamed_frames_survive_underflow_without_callback_allocations() {
	let (mut mixer, mut commands, mut garbage, position) = Mixer::new(48000, 2);
	let (mut writer, stream) = StreamBuffer::bounded(2, 1024).unwrap();
	let data: Vec<f32> = (0..2048).map(|i| if i % 2 == 0 { 0.25 } else { -0.5 }).collect();
	assert_eq!(writer.write(&data), data.len());
	commands
		.push(Command::Stream {
			id: 7,
			stream,
			gain: 1.0,
		})
		.ok()
		.unwrap();
	let mut output = [0.0f32; 2048];
	TRACK.set(true);
	mixer.render(&mut output);
	TRACK.set(false);
	assert_eq!(&output[1000..1004], &[0.25, -0.5, 0.25, -0.5]);
	TRACK.set(true);
	mixer.render(&mut output);
	TRACK.set(false);
	assert!(output.iter().all(|s| *s == 0.0));
	assert_eq!(position.frame.load(std::sync::atomic::Ordering::Relaxed), 1024);
	assert_eq!(writer.write(&data), data.len());
	drop(writer);
	TRACK.set(true);
	mixer.render(&mut output);
	TRACK.set(false);
	assert!(!position.playing.load(std::sync::atomic::Ordering::Acquire));
	assert_eq!(&output[..4], &[0.25, -0.5, 0.25, -0.5]);
	commands.push(Command::Stop).ok().unwrap();
	TRACK.set(true);
	mixer.render(&mut output);
	TRACK.set(false);
	drop(garbage.pop().unwrap());
	assert_eq!(ALLOCS.get(), 0);
	assert_eq!(FREES.get(), 0);
}
