//! Measures allocations only during rewriting (the input is already resident).
use std::alloc::{GlobalAlloc, Layout, System};
use std::io::Cursor;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Tracking;
static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
fn add(size: usize) {
    ALLOCATED.fetch_add(size, Ordering::Relaxed);
    let total = CURRENT.fetch_add(size, Ordering::Relaxed) + size;
    PEAK.fetch_max(total, Ordering::Relaxed);
}
unsafe impl GlobalAlloc for Tracking {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let result = unsafe { System.alloc(layout) };
        if !result.is_null() {
            add(layout.size());
        }
        result
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        CURRENT.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(pointer, layout) };
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let result = unsafe { System.realloc(pointer, layout, size) };
        if !result.is_null() {
            CURRENT.fetch_sub(layout.size(), Ordering::Relaxed);
            add(size);
        }
        result
    }
}
#[global_allocator]
static ALLOCATOR: Tracking = Tracking;

fn webp(size: usize) -> Vec<u8> {
    let image = image::DynamicImage::new_rgb8(32, 32);
    let mut encoded = Cursor::new(Vec::new());
    image
        .write_to(&mut encoded, image::ImageFormat::WebP)
        .unwrap();
    let mut bytes = encoded.into_inner();
    let payload = size.saturating_sub(bytes.len() + 8) & !1;
    bytes.extend_from_slice(b"JUNK");
    bytes.extend_from_slice(&(payload as u32).to_le_bytes());
    bytes.resize(bytes.len() + payload, 0);
    let riff = (bytes.len() - 8) as u32;
    bytes[4..8].copy_from_slice(&riff.to_le_bytes());
    bytes
}
fn avif(size: usize) -> Vec<u8> {
    let mut bytes = include_bytes!("../tests/base.avif").to_vec();
    let payload = size.saturating_sub(bytes.len() + 8);
    bytes.extend_from_slice(&((payload + 8) as u32).to_be_bytes());
    bytes.extend_from_slice(b"free");
    bytes.resize(bytes.len() + payload, 0);
    bytes
}
fn main() {
    let mib: usize = std::env::args()
        .nth(1)
        .unwrap_or("32".into())
        .parse()
        .unwrap();
    for (name, input) in [
        ("webp", webp(mib * 1024 * 1024)),
        ("avif", avif(mib * 1024 * 1024)),
    ] {
        let allocated_before = ALLOCATED.load(Ordering::Relaxed);
        let current = CURRENT.load(Ordering::Relaxed);
        PEAK.store(current, Ordering::Relaxed);
        let start = std::time::Instant::now();
        let output =
            xmp_image::embed_xmp(&input, "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>").unwrap();
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        let peak = PEAK.load(Ordering::Relaxed) - current;
        let allocated = ALLOCATED.load(Ordering::Relaxed) - allocated_before;
        assert!(
            allocated <= output.len() + 128 * 1024,
            "unexpected repeated image allocation: {name} allocated={allocated}"
        );
        assert!(
            peak <= output.len() + 128 * 1024,
            "unexpected whole-image intermediate allocation: {name} peak={peak}"
        );
        println!(
            "{{\"format\":\"{name}\",\"inputBytes\":{},\"rewritePeakBytes\":{peak},\"rewriteAllocatedBytes\":{allocated},\"elapsedMs\":{elapsed:.3}}}",
            input.len()
        );
    }
}
