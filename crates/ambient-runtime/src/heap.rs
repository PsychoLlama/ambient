//! Allocation in WebAssembly.
//!
//! Every program carries its allocator, so this one is small before it
//! is anything else: a free list for each size of block, and memory
//! taken from the engine a stretch at a time. A block that is freed is
//! kept for the next of its size and never joined with its neighbours,
//! so a program's memory is at most what each size took at its peak.
//!
//! TODO: memory is never given back to the engine, and room freed at
//! one size is not reused at another.

use core::alloc::{GlobalAlloc, Layout};
use core::arch::wasm32::memory_grow;

use crate::state::Shared;

/// Every block is a multiple of this many bytes, at an address that is
/// one.
const GRAIN: usize = 16;
/// Sizes up to this many grains have a list each; larger ones are
/// rounded to a power of two.
const EXACT: usize = 64;
/// The powers of two past the exact sizes that an address can reach.
const POWERS: usize = 22;
const PAGE: usize = 1 << 16;
/// The pages taken from the engine at a time, at least.
const STRETCH: usize = 16;

struct Heap {
    /// The first free block of each size, or zero. A free block's
    /// first word is the next.
    free: [usize; EXACT + POWERS],
    /// The memory taken from the engine and not yet handed out.
    next: usize,
    end: usize,
}

// Named, as every state is: a program has one of each name, whatever
// code is loaded into it (`docs/spec/linking.md`).
#[unsafe(export_name = "amb_state_heap")]
static HEAP: Shared<Heap> = Shared::new(Heap {
    free: [0; EXACT + POWERS],
    next: 0,
    end: 0,
});

/// The list a block of `size` bytes is on, and the bytes it takes.
fn class(size: usize) -> (usize, usize) {
    let grains = size.div_ceil(GRAIN).max(1);
    if grains <= EXACT {
        return (grains - 1, grains * GRAIN);
    }
    let bytes = size.next_power_of_two();
    let past = (bytes / (EXACT * GRAIN)).trailing_zeros() as usize;
    (EXACT + past - 1, bytes)
}

impl Heap {
    /// A block of `bytes` that was never handed out, or zero.
    fn fresh(&mut self, bytes: usize) -> usize {
        if self.end - self.next < bytes {
            let pages = bytes.div_ceil(PAGE).max(STRETCH);
            let had = memory_grow(0, pages);
            if had == usize::MAX {
                return 0;
            }
            // What is left of the last stretch is only next to this
            // one if nothing else grew memory in between.
            if had * PAGE != self.end {
                self.next = had * PAGE;
            }
            self.end = (had + pages) * PAGE;
        }
        let block = self.next;
        self.next += bytes;
        block
    }
}

struct Allocator;

#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let (list, bytes) = class(layout.size());
        if layout.align() > GRAIN || list >= EXACT + POWERS {
            return core::ptr::null_mut();
        }
        HEAP.with(|heap| {
            let block = heap.free[list];
            if block == 0 {
                return heap.fresh(bytes) as *mut u8;
            }
            heap.free[list] = unsafe { (block as *const usize).read() };
            block as *mut u8
        })
    }

    unsafe fn dealloc(&self, block: *mut u8, layout: Layout) {
        let (list, _) = class(layout.size());
        HEAP.with(|heap| {
            unsafe { block.cast::<usize>().write(heap.free[list]) };
            heap.free[list] = block as usize;
        });
    }

    unsafe fn realloc(&self, block: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // A block already has room for anything of its size.
        if class(size).1 == class(layout.size()).1 {
            return block;
        }
        unsafe {
            let grown = self.alloc(Layout::from_size_align_unchecked(size, layout.align()));
            if !grown.is_null() {
                core::ptr::copy_nonoverlapping(block, grown, layout.size().min(size));
                self.dealloc(block, layout);
            }
            grown
        }
    }
}
