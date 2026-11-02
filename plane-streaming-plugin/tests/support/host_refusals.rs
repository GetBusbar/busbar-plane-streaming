// SPDX-License-Identifier: Apache-2.0
//! A minimal failing READY callback around the actual streaming door.
use super::*;
use busbar_contract::abi::mechanism::{
    call::{AbiStr, Outcome, RawOutcome},
    door::{Door, DoorParts},
};
use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
static FAILING: AtomicPtr<Door> = AtomicPtr::new(std::ptr::null_mut());
static READY_CALLS: AtomicU64 = AtomicU64::new(0);
extern "C" fn failing() -> *const Door {
    FAILING.load(Ordering::SeqCst)
}
extern "C" fn fail_ready(
    _: *mut std::ffi::c_void,
    _: *const std::ffi::c_void,
    out: *mut std::ffi::c_void,
) -> RawOutcome {
    READY_CALLS.fetch_add(1, Ordering::SeqCst);
    let result = RawOutcome::of(Outcome::Failed);
    // SAFETY: the dispatcher's exclusive READY output is a complete OutHead.
    unsafe {
        let out = &mut *out.cast::<busbar_contract::abi::mechanism::call::OutHead>();
        out.outcome = result;
        out.error = AbiStr::of("streaming discovery refused");
    }
    result
}
#[test]
fn a_failed_ready_refuses_the_actual_streaming_boot_once() {
    let real = busbar_plane_streaming::door().parts();
    // SAFETY: only the optional malicious READY changes; the actual product's static table is retained.
    let door = unsafe {
        Door::__from_parts(DoorParts {
            size: std::mem::size_of::<Door>() as u32,
            ready: Some(fail_ready),
            ..real
        })
    };
    FAILING.store(Box::into_raw(Box::new(door)), Ordering::SeqCst);
    let d = Dispatcher::new(DispatchConfig::default());
    let row = LinkedRow::of_raw(failing).unwrap();
    let p = load_linked::<Plane>(&row, bind(&d)).unwrap();
    let mut f: busbar_plugin_loader::dispatch::Frame<
        busbar_contract::abi::plane::PlaneOpenIn,
        busbar_contract::abi::plane::PlaneOpenOut,
    > = busbar_plugin_loader::dispatch::Frame::new(unsafe { std::mem::zeroed() }, unsafe {
        std::mem::zeroed()
    });
    f.input.open.head = busbar_plugin_loader::dispatch::in_head();
    f.input.open.settings =
        busbar_contract::abi::mechanism::call::Blob::json(br#"{"session":{"model":"m-cap"}}"#);
    f.input.open.generation = 1;
    f.out.open.head = busbar_plugin_loader::dispatch::out_head();
    assert_eq!(
        p.call(
            busbar_contract::abi::mechanism::lifecycle::slot::OPEN,
            &mut f
        )
        .outcome,
        Outcome::Ready
    );
    let first =
        || p.inner.crossings.load(Ordering::SeqCst) - p.inner.resumes.load(Ordering::SeqCst);
    let before = first();
    let error = p
        .ready(&d, std::time::Duration::from_secs(30))
        .expect_err("READY fails");
    assert_eq!(
        error,
        format!(
            "plugin '{}' ready failed: streaming discovery refused",
            p.name()
        )
    );
    assert_eq!(READY_CALLS.load(Ordering::SeqCst), 1);
    assert_eq!(first() - before, 1, "one READY, never retried");
}
