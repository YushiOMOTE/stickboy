#![no_std]
#![no_main]
#![feature(asm)]
#![feature(const_slice_len)]
#![feature(slice_patterns)]

extern crate alloc;

mod gb;

use log::*;
use uefi::prelude::*;

#[no_mangle]
pub extern "C" fn efi_main(_image: uefi::Handle, st: SystemTable<Boot>) -> Status {
    uefi_services::init(&st).expect_success("Failed to initialize utilities");

    st.stdout()
        .reset(false)
        .expect_success("Failed to reset stdout");

    gb::run(st);
}
