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

impl busbar_contract::services::double::ServicesDouble for AdmissionHost {
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
            Admission::Failed => Stored {
                outcome: Outcome::Failed,
                ..Stored::ready(0)
            },
            // A READY scalar outside ticket.take's vocabulary is a broken host response.
            Admission::Broken => Stored::ready(u64::MAX),
        }
    }

    fn random_fill(&self, len: u64) -> busbar_contract::services::Stored {
        self.draws.fetch_add(1, Ordering::Relaxed);
        busbar_contract::services::Stored {
            bytes: vec![0xa0; usize::try_from(len).expect("a random buffer length")],
            ..busbar_contract::services::Stored::ready(0)
        }
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
