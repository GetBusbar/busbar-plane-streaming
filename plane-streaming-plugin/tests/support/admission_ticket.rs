use busbar_contract::services::{
    Caller, DiskDest, HookAsk, Later, NestAsk, Ran, Reading, RecordsList, Snapshot, Stored,
    TrustKeyRef,
};
// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Copy, Debug)]
enum Admission {
    Key,
    Refused,
    Failed,
    Broken,
}

struct AdmissionHost {
    admission: Admission,
    takes: AtomicUsize,
    draws: AtomicUsize,
}

impl busbar_contract::services::HostServices for AdmissionHost {
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
    ) -> Stored {
        Stored::refused(busbar_contract::services::UNSERVED)
    }
    fn trust_decide(
        &self,
        _caller: &Caller,
        _key: TrustKeyRef<'_>,
        _expected: Option<&str>,
        _approve: bool,
    ) -> Stored {
        Stored::refused(busbar_contract::services::UNSERVED)
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
    fn entitlement_check(&self, _caller: &Caller, _unit: Option<u64>, _target: &str) -> Stored {
        Stored::refused(busbar_contract::services::UNSERVED)
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
    ) -> Stored {
        Stored::refused(busbar_contract::services::UNSERVED)
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
    fn snapshot_read(&self, _caller: &Caller, _scope: u32) -> Snapshot {
        Snapshot::Refused(busbar_contract::services::UNSERVED)
    }
    fn ticket_take(
        &self,
        _: &busbar_contract::services::Caller,
        _: Option<u64>,
    ) -> busbar_contract::services::Stored {
        use busbar_contract::abi::host::service::TICKET_NONE;
        use busbar_contract::services::Stored;
        let previous = self.takes.fetch_add(1, Ordering::Relaxed);
        match self.admission {
            Admission::Key if previous == 0 => Stored::ready(TICKET_NONE),
            Admission::Key | Admission::Refused => Stored::refused("admission is not held"),
            Admission::Failed => {
                let mut stored = Stored::ready(0);
                stored.outcome = Outcome::Failed;
                stored
            }

            Admission::Broken => Stored::ready(u64::MAX),
        }
    }

    fn random_fill(&self, len: u64) -> busbar_contract::services::Stored {
        self.draws.fetch_add(1, Ordering::Relaxed);
        let mut stored = busbar_contract::services::Stored::ready(0);
        stored.bytes = vec![0xa0; usize::try_from(len).expect("a random buffer length")];
        stored
    }
}

fn host(admission: Admission) -> (Dispatcher, Arc<AdmissionHost>) {
    let host = Arc::new(AdmissionHost {
        admission,
        takes: AtomicUsize::new(0),
        draws: AtomicUsize::new(0),
    });
    (
        Dispatcher::with_services(DispatchConfig::default(), host.clone()),
        host,
    )
}

fn admission_error_is_refused(projecting: bool) {
    for door in [linked as fn(&Dispatcher) -> Plugin<Plane>, dropped] {
        for admission in [Admission::Refused, Admission::Failed, Admission::Broken] {
            let (dispatcher, host) = host(admission);
            let plugin = door(&dispatcher);
            assert_eq!(open(&plugin, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
            if projecting {
                assert_eq!(
                    project_at(&plugin, 1, 60, b"", 4096).0,
                    Outcome::Refused,
                    "{admission:?} is not a key-admitted unit"
                );
            } else {
                let mut buffers = Bufs::with_units(1 << 16, 8);
                let answer = buffers.call(
                    &plugin,
                    &Push {
                        stream: 61,
                        ..push(61, 1, FROM_CALLER, 0, &[])
                    },
                );
                assert!(answer.starts_with("Refused"), "{admission:?}: {answer}");
            }
            assert_eq!(host.draws.load(Ordering::Relaxed), 0, "no call was placed");
            assert_eq!(host.takes.load(Ordering::Relaxed), 1);
        }
    }
}

#[test]
fn an_admission_service_error_never_opens_an_sdp_session() {
    admission_error_is_refused(false);
}

#[test]
fn an_admission_service_error_never_projects_an_sdp_session() {
    admission_error_is_refused(true);
}

#[test]
fn key_admission_survives_a_short_projection_refresh_and_the_actual_open() {
    const ORIGINAL: &[u8] = br#"{"session":{"model":"m-cap","instructions":"first-generation"}}"#;
    const REPLACEMENT: &[u8] = br#"{"session":{"model":"m-cap","instructions":"next-generation"}}"#;
    for door in [linked as fn(&Dispatcher) -> Plugin<Plane>, dropped] {
        let (dispatcher, host) = host(Admission::Key);
        let plugin = door(&dispatcher);
        assert_eq!(open(&plugin, 1, ORIGINAL, Some(PUBLIC)).0, Outcome::Ready);
        let (short, _, _, recall) = project_recall_at(&plugin, 1, 31, b"", 0, None);
        assert_eq!(short, Outcome::Failed);
        let recall = recall.expect("a short projection can be recalled");
        let mut refresh: Frame<RefreshIn, PlaneRefreshOut> = Frame::new(z(), z());
        refresh.input.head = in_head();
        refresh.input.generation = 2;
        refresh.input.settings = json(REPLACEMENT);
        refresh.out.head = out_head();
        assert_eq!(
            plugin.call(life::REFRESH, &mut refresh).outcome,
            Outcome::Ready
        );
        let (outcome, body, _, _) = project_recall_at(&plugin, 1, 31, b"", 4096, Some(recall));
        assert_eq!(outcome, Outcome::Ready);
        let params: serde_json::Value = serde_json::from_str(&body).expect("projected session");
        assert_eq!(params["instructions"], "first-generation", "{body}");
        let mut buffers = Bufs::with_units(1 << 16, 8);
        let opened = buffers.call(
            &plugin,
            &Push {
                stream: 31,
                ..push(31, 1, FROM_CALLER, 0, &[])
            },
        );
        assert!(
            opened.starts_with("Ready") && opened.contains("status=201"),
            "{opened}"
        );
        let placed = buffers.call(
            &plugin,
            &Push {
                stream: 31,
                ..push(31, 1, FROM_KERNEL, 0, &[])
            },
        );
        assert!(placed.contains("first-generation"), "{placed}");
        assert!(!placed.contains("next-generation"), "{placed}");
        assert_eq!(
            host.takes.load(Ordering::Relaxed),
            1,
            "admission was taken once"
        );
        assert_eq!(host.draws.load(Ordering::Relaxed), 1, "one actual call");
    }
}
