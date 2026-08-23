#![no_std]
#![feature(impl_trait_in_assoc_type)]
#![recursion_limit = "256"]

extern crate alloc;

pub mod util;
pub mod audio;
pub mod sketches;
pub mod input;
pub mod net;
pub mod stream;
pub mod wasm;
pub mod gallery;
pub mod health;
pub mod timebase;

pub fn heap_internal_free() -> usize {
    esp_alloc::HEAP.free_caps(esp_alloc::MemoryCapability::Internal.into())
}

pub fn heap_external_free() -> usize {
    esp_alloc::HEAP.free_caps(esp_alloc::MemoryCapability::External.into())
}
