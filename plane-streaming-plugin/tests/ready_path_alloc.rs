// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! WHAT A READY ARRIVAL COSTS THE HEAP (THE DESIGN section 11.2, "no allocation and no blocking on
//! the hot path"; section 11.4, compiled in or dropped in, "NOTHING CHANGES about the plugin
//! itself").
//!
//! Every door that reaches the far end names its DIRECT route, `streams.session.model`, on every
//! arrival. Naming a route the instance has already kept must cost nothing: the contract SDK keeps a
//! route name once and names the kept copy after, and the plane reads the model where the live
//! generation holds it rather than copying the generation's whole section out per arrival.
//!
//! The door measured here is the one this repository compiles, against the busbar contract this
//! repository pins: the same SDK the dropped-in image (this crate's cdylib) is built from. A cdylib
//! carries its own allocator, so the count is read on the linked door, whose code is that image's.
//!
//! The count is a DIFFERENCE, so the loader's own crossing cost cancels out: an arrival that names
//! the configured model is held to the cost of the same arrival on an instance with no model
//! configured, which names no route and copies no model.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::mem::zeroed;
use std::sync::Arc;

use busbar_contract::abi::mechanism::call::{Blob, Outcome, BLOB_JSON};
use busbar_contract::abi::mechanism::lifecycle::slot as life;
use busbar_contract::abi::plane::{
    slot, ArriveIn, ArriveOut, PlaneOpenIn, PlaneOpenOut, ROUTE_DIRECT, ROUTE_POOL,
};
use busbar_plane_streaming::door;
use busbar_plugin_loader::dispatch::kinds::plane::Plane;
use busbar_plugin_loader::dispatch::{
    in_head, load_linked, out_head, Bind, DispatchConfig, Dispatcher, Frame, LinkedRow, NoSink,
    Plugin,
};

thread_local! {
    /// Allocations made by THIS thread since the counter was last read.
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
}

/// The system allocator, counting.
struct Counting;

// SAFETY: every call is forwarded verbatim to the system allocator; the counter is a thread-local
// `Cell` of a plain integer, touched only on the allocating thread, and never reads or writes the
// memory being handed out.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

fn z<T>() -> T {
    // SAFETY: every `in`/`out` here is plain C data; all-zero is a valid value of each.
    unsafe { zeroed() }
}

/// The section of an instance whose doors name the DIRECT route `m-cap`.
const SESSION: &[u8] = br#"{"session":{"model":"m-cap"}}"#;
/// The section of an instance with no model: its doors name no route.
const BARE: &[u8] = b"{}";
/// The sideband door's claim: a door that reaches the far end.
const SIDEBAND: u32 = 2;

fn linked(d: &Dispatcher, instance: &str, settings: &'static [u8]) -> Plugin<Plane> {
    let row = LinkedRow::of(door).expect("the streaming door states itself");
    let bind = Bind {
        instance: Arc::from(instance),
        max_inflight_cap: 8,
        sink: Arc::new(NoSink),
        dispatcher: d.adopter(),
        // Arrivals only: the door never dials here.
        conns: busbar_plugin_loader::dispatch::ConnTable::Probe,
    };
    let p = load_linked::<Plane>(&row, bind).expect("the linked streaming door loads");
    let mut i: PlaneOpenIn = z();
    i.open.head = in_head();
    i.open.generation = 1;
    i.open.settings = Blob {
        ptr: settings.as_ptr(),
        len: settings.len(),
        fmt: BLOB_JSON,
        flags: 0,
    };
    let mut o: PlaneOpenOut = z();
    o.open.head = out_head();
    let mut f = Frame::new(i, o);
    assert_eq!(p.call(life::OPEN, &mut f).outcome, Outcome::Ready);
    p
}

/// One arrival on `claim`: its outcome, its route class, and the allocations it made.
fn arrive(p: &Plugin<Plane>, claim: u32) -> (Outcome, u8, u64) {
    let mut a: Frame<ArriveIn, ArriveOut> = Frame::new(z(), z());
    a.input.head = in_head();
    a.input.claim = claim;
    a.out.head = out_head();
    let before = ALLOCS.with(Cell::get);
    let outcome = p.call(slot::ARRIVE, &mut a).outcome;
    let made = ALLOCS.with(Cell::get) - before;
    (outcome, a.out.route, made)
}

/// RED-BEFORE-GREEN: a READY arrival that names a route its instance already keeps allocates no
/// more than the same arrival naming none. RED at busbar 10ec8c6b42's contract SDK (`route` keeps
/// `entry.to_string()` under the instance's text mutex on every call) and with the door copying
/// the newest generation's whole section out per arrival.
#[test]
fn naming_a_kept_route_costs_a_ready_arrival_no_allocation() {
    let d = Dispatcher::new(DispatchConfig::default());
    let named = linked(&d, "named", SESSION);
    let bare = linked(&d, "bare", BARE);

    // The first arrival keeps the name; every later one names the kept copy.
    assert_eq!(arrive(&named, SIDEBAND).0, Outcome::Ready);
    assert_eq!(arrive(&bare, SIDEBAND).0, Outcome::Ready);

    for _ in 0..3 {
        let (outcome, route, baseline) = arrive(&bare, SIDEBAND);
        assert_eq!((outcome, route), (Outcome::Ready, ROUTE_POOL));
        let (outcome, route, made) = arrive(&named, SIDEBAND);
        assert_eq!((outcome, route), (Outcome::Ready, ROUTE_DIRECT));
        assert_eq!(
            made, baseline,
            "a READY arrival naming its kept route allocated {made} times, the same arrival naming none {baseline}"
        );
    }
}
