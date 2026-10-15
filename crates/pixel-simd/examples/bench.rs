//! Timings of every path this CPU runs: `cargo run --release -p pixel-simd --example bench`.

fn main() {
  pixel_simd::bench::run();
}
