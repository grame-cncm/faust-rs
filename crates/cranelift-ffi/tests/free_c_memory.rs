//! The `freeCMemory` that `libfaust-rs` exports releases every array a C
//! header documents as freed with it.
//!
//! `interp-ffi`, `box-ffi` and `cranelift-ffi` each define a `freeCMemory`, and
//! only `interp-ffi`'s is exported (`standalone-capi-globals` gates the other
//! two), so a host frees a `Signal*` array from the Box API through the
//! interpreter crate's function. This crate links all three adapters, as the
//! distribution does; calling `interp_ffi::factory::freeCMemory` here is what a
//! host calling `freeCMemory` gets. One test, because the Box API context is
//! process-global.

use std::ffi::{CStr, c_char};

#[test]
fn exported_free_c_memory_releases_signal_and_string_arrays() {
    let mut error = [0 as c_char; 4096];

    // `CboxesToSignals` returns a null-terminated `Signal*` array.
    box_ffi::createLibContext();
    let wires = box_ffi::CboxPar(box_ffi::CboxWire(), box_ffi::CboxWire());
    let signals = unsafe { box_ffi::CboxesToSignals(wires, error.as_mut_ptr()) };
    assert!(!signals.is_null(), "CboxesToSignals failed");
    unsafe {
        assert!(!(*signals).is_null() && !(*signals.add(1)).is_null());
        assert!(
            (*signals.add(2)).is_null(),
            "two signals, then the terminator"
        );
    }
    // The host may destroy the context first: the array stays its to free.
    box_ffi::destroyLibContext();
    unsafe { interp_ffi::factory::freeCMemory(signals.cast()) };
    assert!(
        !ffi_common::free_if_owned_ptr_array(signals.cast()),
        "freeCMemory left the Signal* array allocated"
    );

    // `getAllCInterpreterDSPFactories` returns a null-terminated `char**`:
    // each element, then the array, is freed with `freeCMemory`.
    let factory = unsafe {
        interp_ffi::factory::createCInterpreterDSPFactoryFromString(
            c"free-c-memory".as_ptr(),
            c"process = _;".as_ptr(),
            0,
            std::ptr::null(),
            error.as_mut_ptr(),
        )
    };
    assert!(!factory.is_null(), "{:?}", unsafe {
        CStr::from_ptr(error.as_ptr())
    });
    let keys = interp_ffi::factory::getAllCInterpreterDSPFactories();
    assert!(!keys.is_null());
    unsafe {
        let mut index = 0;
        while !(*keys.add(index)).is_null() {
            assert_eq!(CStr::from_ptr(*keys.add(index)).to_bytes().len(), 40);
            interp_ffi::factory::freeCMemory((*keys.add(index)).cast());
            index += 1;
        }
        assert!(index >= 1);
        interp_ffi::factory::freeCMemory(keys.cast());
    }
    assert!(
        !ffi_common::free_if_owned_ptr_array(keys.cast()),
        "freeCMemory left the char** array allocated"
    );
    unsafe { interp_ffi::factory::deleteCInterpreterDSPFactory(factory) };
}
