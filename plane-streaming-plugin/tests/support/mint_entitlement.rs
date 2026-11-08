// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors
//! Explicit entitlement outcomes for the real streaming door's mint regressions.
use busbar_contract::services::{
    Caller, DiskDest, HookAsk, Later, NestAsk, Ran, Reading, RecordsList, Snapshot, Stored,
    TrustKeyRef,
};
use busbar_plugin_loader::dispatch::{DispatchConfig, Dispatcher};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

#[derive(Clone, Copy)]
pub enum GrantReply {
    Denied,
    Failed,
}
pub struct MintHost {
    reply: GrantReply,
    checks: Mutex<Vec<(Caller, Option<u64>, String)>>,
    mints: AtomicUsize,
}
impl MintHost {
    pub fn assert_checked(&self, instance: &str, unit: u64) {
        let checks = self.checks.lock().unwrap();
        assert_eq!(checks.len(), 1, "one entitlement check");
        let (caller, actual_unit, target) = &checks[0];
        assert_eq!(caller.namespace().as_ref(), instance);
        assert_eq!(
            caller.kind(),
            busbar_contract::abi::mechanism::KindCode::Plane
        );
        assert_eq!(*actual_unit, Some(unit));
        assert_eq!(target, "pool:m-cap");
        assert_eq!(
            self.mints.load(Ordering::Relaxed),
            0,
            "no ticket minted before a successful grant"
        );
    }
}
pub fn dispatcher(reply: GrantReply) -> (Dispatcher, Arc<MintHost>) {
    let host = Arc::new(MintHost {
        reply,
        checks: Mutex::new(Vec::new()),
        mints: AtomicUsize::new(0),
    });
    (
        Dispatcher::with_services(
            DispatchConfig::default(),
            host.clone(),
            busbar_contract::services::test_caller_binding(),
        ),
        host,
    )
}
impl busbar_contract::services::HostServices for MintHost {
    fn now(&self) -> Reading {
        Reading {
            wall_ns: 0,
            mono_ns: 0,
        }
    }
    fn dest_judge(&self, _dest: &str, _class: u32, _flags: u32, _later: Option<Later>) -> Ran {
        Ran::Now(Stored::refused(busbar_contract::services::UNSERVED))
    }
    fn records_get(&self, _caller: &Caller, _kind: &str, _key: &[u8], _later: Later) -> Ran {
        Ran::Now(Stored::refused(busbar_contract::services::UNSERVED))
    }
    fn records_list(&self, _caller: &Caller, _list: RecordsList, _later: Later) -> Ran {
        Ran::Now(Stored::refused(busbar_contract::services::UNSERVED))
    }
    fn records_claim(
        &self,
        _caller: &Caller,
        _kind: &str,
        _key: &[u8],
        _ttl_ms: u64,
        _later: Later,
    ) -> Ran {
        Ran::Now(Stored::refused(busbar_contract::services::UNSERVED))
    }
    fn sign(&self, _caller: &Caller, _data: &[u8]) -> Stored {
        Stored::refused(busbar_contract::services::UNSERVED)
    }
    fn trust_sight(
        &self,
        _caller: &Caller,
        _counterparty: &str,
        _hash: &str,
        _later: Later,
    ) -> Ran {
        Ran::Now(Stored::refused(busbar_contract::services::UNSERVED))
    }
    fn trust_unreached(&self, _caller: &Caller, _counterparty: &str) -> Stored {
        Stored::refused(busbar_contract::services::UNSERVED)
    }
    fn trust_sight_item(
        &self,
        _caller: &Caller,
        _counterparty: &str,
        _item: &str,
        _digest: &str,
        _later: Later,
    ) -> Ran {
        Ran::Now(Stored::refused(busbar_contract::services::UNSERVED))
    }
    fn trust_decide(
        &self,
        _caller: &Caller,
        _ticket: busbar_contract::abi::mechanism::ticket::Ticket,
        _key: TrustKeyRef<'_>,
        _expected: Option<&str>,
        _approve: bool,
        _later: Later,
    ) -> Ran {
        Ran::Now(Stored::refused(busbar_contract::services::UNSERVED))
    }
    fn trust_state(&self, _caller: &Caller, _counterparty: &str) -> Stored {
        Stored::refused(busbar_contract::services::UNSERVED)
    }
    fn trust_serves(
        &self,
        _caller: &Caller,
        _counterparty: &str,
        _item: Option<&str>,
        _digest: Option<&str>,
    ) -> Stored {
        Stored::refused(busbar_contract::services::UNSERVED)
    }
    fn trust_due(&self, _caller: &Caller) -> Stored {
        Stored::refused(busbar_contract::services::UNSERVED)
    }
    fn trust_verify(
        &self,
        _caller: &Caller,
        _counterparty: &str,
        _payload: &[u8],
        _signatures: &[u8],
    ) -> Stored {
        Stored::refused(busbar_contract::services::UNSERVED)
    }
    fn entitlement_check(&self, caller: &Caller, unit: Option<u64>, target: &str) -> Stored {
        self.checks
            .lock()
            .unwrap()
            .push((caller.clone(), unit, target.to_owned()));
        match self.reply {
            GrantReply::Denied => Stored::ready(busbar_contract::abi::host::service::NOT_ENTITLED),
            GrantReply::Failed => {
                let mut answer = Stored::ready(0);
                answer.outcome = busbar_contract::abi::mechanism::call::Outcome::Failed;
                answer
            }
        }
    }
    fn session_emit(&self, _caller: &Caller, _session: u64, _bytes: &[u8]) -> Stored {
        Stored::refused(busbar_contract::services::UNSERVED)
    }
    fn ticket_mint(
        &self,
        _caller: &Caller,
        _unit: Option<u64>,
        _ttl_secs: u64,
        _record: &[u8],
        _later: Later,
    ) -> Ran {
        Ran::Now({
            self.mints.fetch_add(1, Ordering::Relaxed);
            Stored::refused(busbar_contract::services::UNSERVED)
        })
    }
    fn records_secret(&self, _kind: &str, _id: &str, _later: Later) -> Ran {
        Ran::Now(Stored::refused(busbar_contract::services::UNSERVED))
    }
    fn unit_nest(&self, _caller: &Caller, _unit: Option<u64>, _ask: NestAsk, _later: Later) -> Ran {
        Ran::Now(Stored::refused(busbar_contract::services::UNSERVED))
    }
    fn work_open(
        &self,
        _caller: &Caller,
        _unit: Option<u64>,
        _kind: &str,
        _record: &[u8],
        _later: Later,
    ) -> Ran {
        Ran::Now(Stored::refused(busbar_contract::services::UNSERVED))
    }
    fn work_find(
        &self,
        _caller: &Caller,
        _unit: Option<u64>,
        _reference: &[u8],
        _later: Later,
    ) -> Ran {
        Ran::Now(Stored::refused(busbar_contract::services::UNSERVED))
    }
    fn work_settle(
        &self,
        _caller: &Caller,
        _unit: Option<u64>,
        _handle: u64,
        _record: &[u8],
        _later: Later,
    ) -> Ran {
        Ran::Now(Stored::refused(busbar_contract::services::UNSERVED))
    }
    fn work_resume(
        &self,
        _caller: &Caller,
        _unit: Option<u64>,
        _handle: u64,
        _later: Later,
    ) -> Ran {
        Ran::Now(Stored::refused(busbar_contract::services::UNSERVED))
    }
    fn disk_append(&self, _dest: &DiskDest, _bytes: Vec<u8>, _later: Later) -> Ran {
        Ran::Now(Stored::refused(busbar_contract::services::UNSERVED))
    }
    fn verify_lookup(&self, _caller: &Caller, _key: &[u8], _later: Later) -> Ran {
        Ran::Now(Stored::refused(busbar_contract::services::UNSERVED))
    }
    fn verify_store(&self, _caller: &Caller, _key: &[u8], _entry: &[u8], _ttl_ms: u64) -> Stored {
        Stored::refused(busbar_contract::services::UNSERVED)
    }
    fn content_scan(
        &self,
        _caller: &Caller,
        _unit: Option<u64>,
        _content: &[u8],
        _later: Later,
    ) -> Ran {
        Ran::Now(Stored::refused(busbar_contract::services::UNSERVED))
    }
    fn hook_call(&self, _caller: &Caller, _unit: Option<u64>, _ask: HookAsk, _later: Later) -> Ran {
        Ran::Now(Stored::refused(busbar_contract::services::UNSERVED))
    }
    fn snapshot_read(&self, _caller: &Caller, _scope: u32, _later: Later) -> Ran {
        Ran::Now((Snapshot::Refused(busbar_contract::services::UNSERVED)).stored())
    }
    fn ticket_take(&self, _caller: &Caller, _unit: Option<u64>) -> Stored {
        Stored::refused(busbar_contract::services::UNSERVED)
    }
    fn random_fill(&self, _len: u64) -> Stored {
        Stored::refused(busbar_contract::services::UNSERVED)
    }
}
