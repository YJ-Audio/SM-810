fn main() {
	let args: Vec<_> = std::env::args_os().skip(1).collect();
	if args.len() != 2 {
		eprintln!("usage: sampler-layout input.bin output.bin");
		std::process::exit(2);
	}
	let result = (|| -> Result<(), sampler_layout::Error> {
		let (store, dimensions) = sampler_layout::read_input(std::path::Path::new(&args[0]))?;
		let points = sampler_layout::project(&store, dimensions)?;
		sampler_layout::write_output(std::path::Path::new(&args[1]), &points)
	})();
	if let Err(error) = result {
		eprintln!("{error}");
		std::process::exit(1);
	}
}
