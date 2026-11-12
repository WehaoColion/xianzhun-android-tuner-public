// Run from the project root:
// rustc --edition=2021 -O rust/tools/pitch_benchmark.rs -o <temporary-executable>
// <temporary-executable>
#[allow(dead_code)]
#[path = "../src/pitch.rs"]
mod pitch;

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

struct CountAllocator;
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        System.dealloc(pointer, layout);
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        System.realloc(pointer, layout, size)
    }
}

#[global_allocator]
static ALLOCATOR: CountAllocator = CountAllocator;

fn main() {
    let tones: Vec<Vec<i16>> = [82.41_f32, 110.0, 146.83, 196.0, 246.94, 329.63]
        .iter()
        .map(|&frequency| {
            (0..4096)
                .map(|index| {
                    let phase = std::f32::consts::TAU * frequency * index as f32 / 48_000.0;
                    ((0.1 * phase.sin() + 0.3 * (2.0 * phase).sin() + 0.1 * (3.0 * phase).sin())
                        * i16::MAX as f32) as i16
                })
                .collect()
        })
        .collect();
    let mut analyzer = pitch::PitchAnalyzer::default();
    for samples in &tones {
        black_box(analyzer.analyze_pcm(black_box(samples), 48_000.0));
    }
    ALLOCATIONS.store(0, Ordering::Relaxed);
    let started = Instant::now();
    let mut checksum = 0.0;
    for _ in 0..400 {
        for samples in &tones {
            let reading = black_box(analyzer.analyze_pcm(black_box(samples), 48_000.0)).unwrap();
            checksum += reading.frequency_hz;
        }
    }
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    let allocations = ALLOCATIONS.load(Ordering::Relaxed);
    println!(
        "2400 guitar frames: {elapsed_ms:.3} ms, checksum {checksum:.3}, warm allocations {allocations}"
    );
}
