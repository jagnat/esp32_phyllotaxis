#![no_std]

mod api;
mod game;

use api::{Frame, Input, FRAME_BYTES, INPUT_BYTES};

static mut INPUT: [u8; INPUT_BYTES] = [0; INPUT_BYTES];
static mut FRAME: [u8; FRAME_BYTES] = [0; FRAME_BYTES];
static mut GAME: game::Game = game::Game::new();

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

#[unsafe(no_mangle)]
pub extern "C" fn abi_version() -> u32 {
    api::ABI_VERSION
}

#[unsafe(no_mangle)]
pub extern "C" fn caps() -> u32 {
    api::INPUT_CAPABILITY
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
    let input = Input::new(unsafe { &*core::ptr::addr_of!(INPUT) });
    let mut frame = Frame::new(unsafe { &mut *core::ptr::addr_of_mut!(FRAME) });
    unsafe { (&mut *core::ptr::addr_of_mut!(GAME)).render(&input, &mut frame) };
}

// Gallery hover uses a non-mutating preview so it cannot consume lives.
#[unsafe(no_mangle)]
pub extern "C" fn preview() {
    let input = Input::new(unsafe { &*core::ptr::addr_of!(INPUT) });
    let mut frame = Frame::new(unsafe { &mut *core::ptr::addr_of_mut!(FRAME) });
    game::preview(&input, &mut frame);
}
