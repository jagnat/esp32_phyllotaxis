#![no_std]

mod phyllo;
mod sketch;

use phyllo::{Frame, Input, FRAME_BYTES, INPUT_BYTES};

static mut INPUT: [u8; INPUT_BYTES] = [0; INPUT_BYTES];
static mut FRAME: [u8; FRAME_BYTES] = [0; FRAME_BYTES];

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

// Everything below is ABI glue. Sketch authors normally edit only sketch.rs.

#[unsafe(no_mangle)]
pub extern "C" fn abi_version() -> u32 {
    phyllo::ABI_VERSION
}

#[unsafe(no_mangle)]
pub extern "C" fn caps() -> u32 {
    sketch::CAPABILITIES
}

#[unsafe(no_mangle)]
pub extern "C" fn input_ptr() -> i32 {
    core::ptr::addr_of!(INPUT) as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn frame_ptr() -> i32 {
    core::ptr::addr_of!(FRAME) as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn render() {
    // The host owns these buffers between calls. The runtime keeps the unsafe
    // boundary here so sketch.rs receives ordinary safe Rust types.
    let input = Input::new(unsafe { &*core::ptr::addr_of!(INPUT) });
    let mut frame = Frame::new(unsafe { &mut *core::ptr::addr_of_mut!(FRAME) });
    sketch::render(&input, &mut frame);
}
