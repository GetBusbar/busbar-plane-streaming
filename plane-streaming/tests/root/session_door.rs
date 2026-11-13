// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

#[cfg(linked_section_streams)]
pub(crate) mod session_door {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use axum::http::StatusCode;
    use busbar_contract::caps::ReasonCode;
    use busbar_contract::conn::{DeclaredConns, PollConns};
    use busbar_kernel::cost::CostModel;
    use busbar_kernel::governance::signing::{TokenSigner, DEFAULT_KID};
    use busbar_kernel::governance::{GovState, NewKeySpec};
    use busbar_kernel::plane_driver::{refusal_status, EndPost, PlaneMoney};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use crate::root::door_steps::{provider_routes, DoorReach, OutboundAuths};
    use crate::root::loader::dispatch::kinds::plane::Plane;
    use crate::root::loader::dispatch::{
        load_linked, Bind, DispatchConfig, Dispatcher, LinkedRow, NoSink,
    };
    use crate::root::plane_node::{Node, NodeEndPost};
    use crate::root::serve::planes_tests::{composed_services, PUBLISHING};
    use crate::root::serve::{compose_planes, DoorEgress};

    static CARD: std::sync::LazyLock<crate::root::kernel::RootHistory> =
        std::sync::LazyLock::new(|| {
            let holder = crate::root::kernel::RootHistory::default();
            holder.apply(
                crate::root::kernel::card_from_config(
                    std::iter::empty::<(&str, busbar_contract::billing::RawTierRates)>(),
                    0,
                    true,
                ),
                1_000,
            );
            holder
        });

    const CREDENTIAL: &str = "sk-door-test";

    const MINT: &str = "/v1/realtime/client_secrets";

    const SIDEBAND: &str = "/v1/realtime/sideband/{call_id}";

    const REALTIME: &str = "/v1/realtime?model=m-cap";

    const SIGNAL: &str = "/v1/realtime/calls";
    const PROVIDER_CALL_ID: &str = "rtc_provider0001";

    const PUBLIC: &str = "http://gw.test";

    const AUDIO_CLASS: &str = "audio_seconds_in";

    async fn send_body(
        router: &axum::Router,
        path: &str,
        token: Option<&str>,
        body: &'static str,
    ) -> axum::response::Response {
        use tower::ServiceExt as _;
        let mut req = axum::http::Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json");
        if let Some(token) = token {
            req = req.header("authorization", format!("Bearer {token}"));
        }
        let req = req.body(axum::body::Body::from(body)).expect("a request");
        tokio::time::timeout(ANSWER_BOUND, router.clone().oneshot(req))
            .await
            .unwrap_or_else(|_| panic!("the door answers {path} within {ANSWER_BOUND:?}"))
            .expect("the router answers")
    }

    const ANSWER_BOUND: std::time::Duration = std::time::Duration::from_secs(15);

    async fn body_of(response: axum::response::Response) -> axum::body::Bytes {
        tokio::time::timeout(
            ANSWER_BOUND,
            axum::body::to_bytes(response.into_body(), 1 << 16),
        )
        .await
        .expect("the body ends within the bound")
        .expect("the body")
    }

    /// How the far end answers one connection.
    #[derive(Debug, Clone, Copy)]
    enum Far {
        /// A status line and a JSON body.
        Says(&'static str, &'static str),
        /// Nothing: the request is read and the connection held open, unanswered.
        Holds,
        Rtc,
    }

    type FarScript = Arc<dyn Fn(usize) -> Far + Send + Sync>;

    use crate::root::serve::serve_datagram::peer::Peer as RtcPeer;

    #[derive(Default)]
    struct RtcTasks(std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>);
    impl RtcTasks {
        fn keep(&self, task: tokio::task::JoinHandle<()>) {
            self.0.lock().expect("the peer tasks").push(task);
        }
    }
    impl Drop for RtcTasks {
        fn drop(&mut self) {
            let tasks = std::mem::take(self.0.get_mut().expect("the peer tasks"));
            for task in &tasks {
                task.abort();
            }

            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    for task in tasks {
                        let _ = task.await;
                    }
                });
            }
        }
    }

    struct MediaStop(tokio::sync::watch::Sender<bool>);
    impl Drop for MediaStop {
        fn drop(&mut self) {
            let _ = self.0.send(true);
        }
    }

    fn provider_offer(request: &str) -> Vec<u8> {
        let (head, body) = request
            .split_once("\r\n\r\n")
            .expect("the provider HTTP head");
        let boundary = head
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-type")
                    .then(|| value.split("boundary=").nth(1))
                    .flatten()
            })
            .expect("the product multipart boundary")
            .trim()
            .trim_matches('"');
        let marker = format!("--{boundary}");
        let parts: Vec<_> = body.split(&marker).collect();
        assert_eq!(
            parts.len(),
            4,
            "the actual provider form has exactly two parts"
        );
        assert!(parts[0].is_empty(), "the form starts at its boundary");
        assert!(parts[3] == "--\r\n", "the form closes its actual boundary");
        let (session_fields, session) = parts[1]
            .strip_prefix("\r\n")
            .expect("the session part delimiter")
            .split_once("\r\n\r\n")
            .expect("the session part head");
        assert!(session_fields
            .lines()
            .any(|line| line == "Content-Disposition: form-data; name=\"session\""));
        assert!(session_fields
            .lines()
            .any(|line| line.eq_ignore_ascii_case("Content-Type: application/json")));
        let session: serde_json::Value =
            serde_json::from_str(session.strip_suffix("\r\n").expect("the session part ends"))
                .expect("the actual locked session is JSON");
        assert!(session.is_object(), "the locked session is an object");
        let (sdp_fields, sdp) = parts[2]
            .strip_prefix("\r\n")
            .expect("the SDP part delimiter")
            .split_once("\r\n\r\n")
            .expect("the SDP part head");
        assert!(sdp_fields
            .lines()
            .any(|line| line == "Content-Disposition: form-data; name=\"sdp\""));
        assert!(sdp_fields
            .lines()
            .any(|line| line.eq_ignore_ascii_case("Content-Type: application/sdp")));
        sdp.strip_suffix("\r\n")
            .expect("the SDP part ends")
            .as_bytes()
            .to_vec()
    }

    async fn far_end_scripted(
        script: FarScript,
        connector: Arc<busbar_core_connector::Connector>,
        mut stopped: tokio::sync::watch::Receiver<bool>,
        peers: std::sync::Weak<RtcTasks>,
    ) -> (u16, tokio::sync::mpsc::UnboundedReceiver<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port");
        let port = listener.local_addr().expect("its address").port();
        let (sent, heard) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut accepted = 0usize;
            loop {
                let accepted_socket = tokio::select! {
                    got = listener.accept() => got,
                    _ = stopped.changed() => return,
                };
                let Ok((mut socket, _)) = accepted_socket else {
                    return;
                };
                let answer = script(accepted);
                accepted += 1;
                let sent = sent.clone();
                let connector = Arc::clone(&connector);
                let mut stopped = stopped.clone();
                let peers = peers.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 16 * 1024];
                    let mut got = Vec::new();

                    loop {
                        let Ok(n) = socket.read(&mut buf).await else {
                            return;
                        };
                        if n == 0 {
                            return;
                        }
                        got.extend_from_slice(&buf[..n]);
                        let text = String::from_utf8_lossy(&got).to_string();
                        if let Some(at) = text.find("\r\n\r\n") {
                            let length = text[..at]
                                .lines()
                                .find_map(|l| {
                                    let (name, value) = l.split_once(':')?;
                                    name.eq_ignore_ascii_case("content-length")
                                        .then(|| value.trim().parse::<usize>().ok())
                                        .flatten()
                                })
                                .unwrap_or(0);
                            if got.len() >= at + 4 + length {
                                break;
                            }
                        }
                    }
                    let request = String::from_utf8_lossy(&got);
                    let websocket_key = request.lines().find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("sec-websocket-key")
                            .then(|| value.trim().to_owned())
                    });
                    if matches!(&answer, Far::Rtc) {
                        if let Some(key) = websocket_key {
                            let _ = sent.send(format!(
                                "DIAL {}",
                                request.lines().next().unwrap_or_default()
                            ));
                            let response = format!(
                                "HTTP/1.1 101 Switching Protocols\r\nupgrade: websocket\r\nconnection: Upgrade\r\nsec-websocket-accept: {}\r\n\r\n",
                                ws_accept(&key)
                            );
                            if socket.write_all(response.as_bytes()).await.is_err() {
                                return;
                            }
                            if socket.write_all(&server_text(FAR_OPENING)).await.is_err() {
                                return;
                            }
                            while let Some((opcode, payload)) = read_frame(&mut socket).await {
                                match opcode {
                                    0x1 => {
                                        let _ = sent.send(format!(
                                            "TEXT {}",
                                            String::from_utf8_lossy(&payload)
                                        ));
                                    }
                                    0x8 => return,
                                    _ => {}
                                }
                            }
                            return;
                        }
                    }
                    let _ = sent.send(request.to_string());
                    let (line, body) = match answer {
                        Far::Says(line, body) => (line, body.to_owned()),
                        Far::Rtc => {
                            let request = String::from_utf8_lossy(&got);
                            let offer = provider_offer(&request);
                            let (peer, answer) = RtcPeer::accept(
                                connector
                                    .framer_for("webrtc")
                                    .expect("the actual provider WebRTC framer"),
                                &offer,
                            )
                            .await;
                            let (task, _) = peer.run(stopped, Some(FAR_OPENING.as_bytes()));
                            peers
                                .upgrade()
                                .expect("the provider fixture is retained")
                                .keep(task);
                            (
                                "201 Created",
                                String::from_utf8(answer)
                                    .expect("the actual provider SDP is UTF-8"),
                            )
                        }
                        Far::Holds => {
                            tokio::select! {
                                () = tokio::time::sleep(std::time::Duration::from_secs(3600)) => {},
                                _ = stopped.changed() => {},
                            }
                            return;
                        }
                    };
                    let kind = if body.starts_with("v=0") {
                        "application/sdp"
                    } else {
                        "application/json"
                    };
                    let location = if body.starts_with("v=0") {
                        format!("location: {SIGNAL}/{PROVIDER_CALL_ID}\r\n")
                    } else {
                        String::new()
                    };
                    let reply = format!(
                        "HTTP/1.1 {line}\r\ncontent-type: {kind}\r\n{location}content-length: {}\r\n\
                         connection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(reply.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        (port, heard)
    }

    async fn global_gate(
        reply: serde_json::Value,
        armed: Arc<std::sync::atomic::AtomicBool>,
    ) -> busbar_kernel::config::HookCfg {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("the hook endpoint binds");
        let url = format!("http://{}/hook", listener.local_addr().unwrap());
        let router = axum::Router::new().route(
            "/hook",
            axum::routing::post(move || {
                let reply = reply.clone();
                let armed = Arc::clone(&armed);
                async move {
                    use axum::response::IntoResponse as _;
                    if armed.load(std::sync::atomic::Ordering::SeqCst) {
                        axum::Json(reply).into_response()
                    } else {
                        StatusCode::NO_CONTENT.into_response()
                    }
                }
            }),
        );
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let path = std::path::PathBuf::from(
            std::env::var_os("BUSBAR_TEST_WEBREQUEST_CDYLIB")
                .expect("BUSBAR_TEST_WEBREQUEST_CDYLIB names the actual built hook"),
        );
        let statement = crate::root::loader::dispatch::rendering_of_library(&path)
            .expect("the actual hook statement reads")
            .expect("the actual hook exports its statement");
        let module = busbar_contract::abi::mechanism::rendering::read(&statement)
            .expect("the actual hook statement is valid")
            .name;
        serde_json::from_value(serde_json::json!({
            "kind": "gate",
            "module": module,
            "timeout_ms": 10_000,
            "on_error": "weighted",
            "prompt": "ro",
            "user": "ro",
            "priority": 0,
            "settings": { "url": url },
            "global": true,
            "default": false,
            "signals": [],
            "groups": [],
            "phase": [],
        }))
        .expect("a hook document")
    }

    struct MediaPeer {
        ready: bool,
        received: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
    }

    pub(in crate::root::serve) struct SessionDoor {
        _handle: Arc<busbar_kernel::state::AppHandle>,
        framer: crate::root::listener::framed::DataFramer,
        connector: Arc<busbar_core_connector::Connector>,
        media: MediaStop,
        peers: Arc<RtcTasks>,
        media_messages: tokio::sync::Mutex<Vec<MediaPeer>>,
        router: axum::Router,
        heard: tokio::sync::mpsc::UnboundedReceiver<String>,
        gov: Arc<GovState>,
        pub(in crate::root::serve) app: Arc<busbar_kernel::state::App>,
        key: busbar_contract::records::VirtualKey,

        plain: String,

        pub(in crate::root::serve) live: Arc<crate::root::serve::DoorApply>,
        money: Arc<PlaneMoney>,
        post: Arc<NodeEndPost>,
        book: crate::root::durability::NodeBook,
    }

    /// How a cell composes the session door beyond the defaults ([`session_with`]).
    #[derive(Default)]
    struct Opts {
        /// The caller's group's all-time budget, with the door's per-request fee one; `None` = no
        /// budget and no fee.
        budget: Option<u64>,
        /// Further limits on the caller's group (a group is made for them when there is none).
        limits: Vec<busbar_kernel::config::groups::LimitCfg>,
        /// The pools the caller's key is minted with; `None` = every pool.
        allowed_pools: Option<Vec<String>>,
        /// How the provider answers; `None` = an accepted SDP answer.
        far: Option<FarScript>,
        /// Hooks the deployment defines, each `global` (run on every plane's units), with the hook
        /// stage composed over the generation; empty = no hook stage.
        hooks: Vec<(&'static str, busbar_kernel::config::HookCfg)>,
        /// A provider the cell started itself (its port, and what it reports hearing), in place of
        /// the scripted one.
        far_end: Option<(u16, tokio::sync::mpsc::UnboundedReceiver<String>)>,
        /// The provider's wire name for the catalog model the section's `session.model` names
        /// (`upstream_model`, Part 2 #51), written in the deployment's `models:` as production
        /// writes it; `None` = the catalog name is the wire name.
        upstream_model: Option<&'static str>,
        /// The deployment configures no media listener (`media.listen`): no claim door is bound.
        no_media: bool,
        oauth_admin: Option<bool>,
    }

    fn session_door(
        instance: &'static str,
        dispatcher: &Arc<Dispatcher>,
        conns: Arc<dyn DeclaredConns>,
    ) -> Option<crate::root::loader::dispatch::Plugin<Plane>> {
        crate::LINKED.plane_doors.iter().find_map(|door| {
            let row = LinkedRow::of(*door).expect("the door states its Statement");
            let plane = load_linked::<Plane>(
                &row,
                Bind {
                    instance: Arc::from(instance),
                    max_inflight_cap: 64,
                    sink: Arc::new(NoSink),
                    dispatcher: dispatcher.adopter(),
                    conns: crate::root::loader::dispatch::ConnTable::Host(
                        crate::root::connector::admit(Arc::clone(&conns)),
                    ),
                },
            )
            .expect("the linked door binds");
            plane
                .served()
                .billable_classes
                .contains(&AUDIO_CLASS)
                .then_some(plane)
        })
    }

    fn connector() -> Arc<busbar_core_connector::Connector> {
        let judge = crate::root::connector::guard_for(&busbar_kernel::config::Destinations {
            block_private_addresses: false,
            ..Default::default()
        })
        .expect("the guard");
        busbar_core_connector::process::build_with_secrets(
            || {
                crate::root::connector::entries(
                    crate::LINKED_TRANSPORT_DOORS,
                    &busbar_contract::transport::TransportSettings::default(),
                )
            },
            judge,
            &[],
            Arc::new(|_, _| {}),
            busbar_core_connector::pool::PoolPosture::NONE,
            busbar_core_connector::spawn_env::linked_spawn_secret,
        )
        .expect("the connector builds")
    }

    pub(in crate::root::serve) fn name() -> Option<String> {
        let _one = PUBLISHING.blocking_lock();
        let instance = "session-door-name";
        let dispatcher = Arc::new(Dispatcher::new(
            DispatchConfig::default(),
            busbar_kernel::host_services::caller_binding(),
        ));
        session_door(instance, &dispatcher, connector() as Arc<dyn DeclaredConns>)
            .map(|plane| plane.name().to_owned())
    }

    async fn session_served(instance: &'static str, budget: Option<u64>) -> Option<SessionDoor> {
        session_with(
            instance,
            Opts {
                budget,
                ..Opts::default()
            },
        )
        .await
    }

    async fn session_with(instance: &'static str, opts: Opts) -> Option<SessionDoor> {
        let key_file = std::env::temp_dir().join(format!(
            "busbar-serve-door-{instance}-{}",
            std::process::id()
        ));
        std::fs::write(&key_file, CREDENTIAL).expect("the credential file");
        let api_key = format!("{{file: '{}'}}", key_file.display());
        let served = session_composed(instance, opts, &api_key).await;
        let _ = std::fs::remove_file(&key_file);
        served.map(|s| {
            s.unwrap_or_else(|e| panic!("the session door composes, its egress sealed: {e}"))
        })
    }

    pub(in crate::root::serve) async fn session_keyed(
        instance: &'static str,
        budget: Option<u64>,
        api_key: &str,
    ) -> Option<Result<SessionDoor, String>> {
        session_composed(
            instance,
            Opts {
                budget,
                ..Opts::default()
            },
            api_key,
        )
        .await
    }

    async fn session_composed(
        instance: &'static str,
        opts: Opts,
        api_key: &str,
    ) -> Option<Result<SessionDoor, String>> {
        crate::dist_root_install::install();
        busbar_kernel::snapshot::init_for_test(std::time::Duration::from_secs(10));
        let budget = opts.budget;
        let _ = crate::root::inbound::install(crate::root::doors::dispatcher());

        let connector = Arc::clone(
            crate::root::connector::installed()
                .expect("normal root bootstrap installs the session connector"),
        );
        assert!(Arc::ptr_eq(&connector, crate::root::connector::the()));
        let (media_stop, media_stopped) = tokio::sync::watch::channel(false);
        let peers = Arc::new(RtcTasks::default());
        let (port, heard) = match (opts.far_end, opts.far) {
            (Some(far_end), _) => far_end,
            (None, script) => {
                far_end_scripted(
                    script.unwrap_or_else(|| Arc::new(|_| Far::Rtc)),
                    Arc::clone(&connector),
                    media_stopped.clone(),
                    Arc::downgrade(&peers),
                )
                .await
            }
        };
        let services = composed_services();
        let dispatcher = Arc::new(crate::root::dispatch::build(1, &services));
        let plane = session_door(
            instance,
            &dispatcher,
            Arc::clone(&connector) as Arc<dyn DeclaredConns>,
        )?;
        let plane_name = plane.name().to_owned();
        let section_key = plane.served().section;

        let protocol = {
            let served = plane.served();
            served
                .dialect_auth
                .iter()
                .rev()
                .filter(|(_, style, _)| *style == "bearer")
                .find_map(|(d, _, _)| {
                    usize::try_from(*d)
                        .ok()
                        .and_then(|i| served.dialects.get(i).copied())
                })
                .expect("the session door states a bearer-style dialect")
        };

        let mut limits = opts.limits;
        if let Some(amount) = budget {
            limits.push(busbar_kernel::config::groups::LimitCfg {
                metric: busbar_kernel::config::groups::LimitMetric::Budget,

                amount: amount.max(1),
                per: Some(busbar_kernel::config::groups::LimitWindow::Total),
                scope: None,
                on_exhaust: None,
                downgrade_to: None,
                admission: None,
                on_exhaustion: None,
            });
        }
        let groups: BTreeMap<String, busbar_kernel::config::GroupCfg> = if limits.is_empty() {
            BTreeMap::new()
        } else {
            let cfg = busbar_kernel::config::GroupCfg {
                parent: None,
                enabled: true,
                limits,
                ..Default::default()
            };
            BTreeMap::from([(format!("{instance}-group"), cfg)])
        };
        let cost = match budget {
            None if groups.is_empty() => CostModel::flat(1),
            None => CostModel::resolve_parts(None, 1, &groups),
            Some(_) => {
                let fees: busbar_kernel::config::PlaneFeesMap = [(
                    plane_name.clone(),
                    busbar_kernel_ledger::cost::PlaneFees {
                        per_request: 0,
                        per_session: 1,
                    },
                )]
                .into_iter()
                .collect();
                CostModel::resolve_parts(None, 0, &groups).with_plane_fees(&fees)
            }
        };
        let signer = TokenSigner::from_secret_bytes(&[7u8; 32], DEFAULT_KID);
        let gov = Arc::new(
            GovState::new_with_core(
                crate::root::serve::door_tests::process_store().await,
                None,
                Some(signer),
            )
            .await
            .expect("governance"),
        );
        crate::root::serve::door_rig::attach_late(&services, &gov);
        let (key, plain) = gov
            .mint_signed_async(
                NewKeySpec {
                    name: "browser".to_string(),
                    group: groups.keys().next().cloned(),
                    allowed_pools: opts.allowed_pools,
                    ..Default::default()
                },
                4_000_000_000,
                1_700_000_000,
            )
            .await
            .expect("mint");
        gov.hydrate_budgets_async(&cost, 0).await.expect("hydrate");
        let node = Arc::new(Node::with_inbound_keys());
        let mut book = crate::root::durability::node_book(
            busbar_kernel::persistence::journal::runtime::RuntimeJournal::open(
                gov.core_store_handle().expect("native Store"),
                busbar_kernel::door::op_id().counter().max(1),
                busbar_kernel::persistence::journal::runtime::BufferBudget {
                    records: 4096,
                    bytes: 16 * 1024 * 1024,
                },
            )
            .await
            .expect("native journal"),
            busbar_kernel::persistence::journal::runtime::BufferBudget {
                records: 4096,
                bytes: 16 * 1024 * 1024,
            },
        )
        .await
        .expect("native node book");
        crate::root::serve::door_tests::bind_process_amendments(
            &mut book.durability,
            gov.amendments(),
        )
        .await;
        node.bind_book(Arc::clone(&book.durability));
        let post = Arc::new(NodeEndPost::new(Arc::clone(&node)));
        let money = Arc::new(PlaneMoney::new(
            Arc::clone(&gov),
            Arc::clone(&post) as Arc<dyn EndPost>,
        ));
        let one = Arc::clone(&money);

        let provider: busbar_kernel::config::ProviderCfg = serde_yaml::from_str(&format!(
            "{{protocol: {protocol}, base_url: 'http://127.0.0.1:{port}', api_key: {api_key}, error_map: {{}}}}",
        ))
        .expect("a provider entry");
        let providers = provider_routes(
            &std::collections::HashMap::from([(
                "p".to_string(),
                busbar_kernel::config::ResolvedProvider::from(provider),
            )]),
            &busbar_kernel::config_validate::MetadataPosture::default(),
        );
        let models_yaml = match opts.upstream_model {
            Some(wire) => format!("{{m-cap: {{provider: p, upstream_model: {wire}}}}}"),
            None => "{m-cap: {provider: p}}".to_string(),
        };
        let catalog: std::collections::HashMap<String, busbar_contract::config::ModelCfg> =
            serde_yaml::from_str(&models_yaml).expect("the models catalog");
        #[cfg(distribution_root)]
        crate::dist_root_install::install();
        let secrets: Arc<dyn busbar_contract::secret::SecretResolve> =
            Arc::new(busbar_kernel::config::secret::SecretResolver::builtins_only());
        let auths = OutboundAuths::new(
            Arc::clone(&dispatcher),
            crate::LINKED.auths,
            None,
            crate::root::loader::dispatch::ConnTable::Host(crate::root::connector::admit(
                Arc::clone(&connector) as Arc<dyn DeclaredConns>,
            )),
            busbar_kernel::diagnostics::Catalogue::NEUTRAL,
        );
        let reach = DoorReach {
            providers: &providers,
            secrets: Arc::clone(&secrets),
            auths: Arc::new(auths),
            conns: Arc::clone(&connector) as Arc<dyn PollConns>,
            stream_ceiling_secs: 600,
            catalog: Some(&catalog),
            models: None,

            upgrades: crate::root::serve::upgrade_carriers(crate::LINKED.transports, &[]),
        };
        let mut sections = BTreeMap::new();
        sections.insert(
            section_key,
            serde_yaml::from_str(if opts.no_media {
                "{session: {model: m-cap}}"
            } else {
                "{session: {model: m-cap}, media: {listen: '127.0.0.1:0'}}"
            })
            .expect("a section"),
        );

        sections.insert(
            busbar_contract::section::RESERVED_MODELS_KEY,
            serde_yaml::from_str(&models_yaml).expect("the models section"),
        );

        let live_app: Arc<std::sync::OnceLock<std::sync::Weak<busbar_kernel::state::AppHandle>>> =
            Arc::default();
        let stage = (!opts.hooks.is_empty()).then(|| crate::root::serve::HookStage {
            host: {
                let live_app = Arc::clone(&live_app);
                Arc::new(move || {
                    let handle = live_app
                        .get()
                        .and_then(std::sync::Weak::upgrade)
                        .expect("the session AppHandle outlives its requests");
                    busbar_kernel::plane_driver::engine_host(&handle.load())
                })
            },
            gov: Arc::clone(&gov),
            journal: Some(Arc::clone(&post) as Arc<dyn busbar_kernel::plane_driver::HookJournal>),
        });

        let tables = busbar_kernel::plane_driver::serve::ServeTables::default();
        let doors = [(instance.to_string(), Arc::new(plane))];
        let media_binds = crate::root::serve::datagram_binds(&doors, &sections)
            .expect("the real deployment's datagram binds");
        if opts.no_media {
            assert!(
                media_binds.is_empty(),
                "no media declaration binds no claim door"
            );
        } else {
            assert!(
                media_binds
                    .iter()
                    .any(|bind| bind.transport == "webrtc" && bind.listen.ip().is_loopback()),
                "the actual media declaration binds a loopback WebRTC claim door"
            );
        }
        let mut served = match compose_planes(
            &doors,
            &dispatcher,
            &services,
            &sections,
            Some(PUBLIC),
            &move || Arc::clone(&one),
            Some(&DoorEgress {
                reach: &reach,
                journal: Arc::clone(&post) as Arc<dyn busbar_kernel_egress::ports::Journal>,
            }),
            stage.as_ref(),
            &tables,
        )
        .await
        {
            Ok(served) => served,
            Err(refusal) => return Some(Err(refusal)),
        };
        assert!(
            served.planes[0].live.current().egress.is_some(),
            "the composition sealed its egress"
        );
        let runtime = tokio::runtime::Handle::current();
        let media_connector = Arc::clone(&connector);
        let stopped = media_stopped.clone();
        let tracker = busbar_kernel::detached::worker_detached();
        served.claim_doors = std::thread::spawn(move || {
            let _entered = runtime.enter();
            busbar_kernel::detached::set_worker_shutdown(stopped);
            if let Some(tracker) = tracker {
                busbar_kernel::detached::set_worker_detached(tracker);
            }
            crate::root::serve::serve_datagram::ClaimDoors::bind(&media_binds, |scheme| {
                media_connector.framer_for(scheme)
            })
        })
        .join()
        .expect("the ordinary media worker initializes")
        .expect("the actual media claim doors bind");
        if let Some(shutdown) = busbar_kernel::detached::worker_shutdown() {
            let stop = media_stop.clone();
            let mut stopped = media_stopped.clone();
            tokio::spawn(async move {
                tokio::select! {
                    () = busbar_kernel::detached::shutdown_fired(Some(shutdown)) => { let _ = stop.send(true); },
                    _ = stopped.changed() => {},
                }
            });
        }
        let live = Arc::clone(&served.planes[0].live);
        served.post = Some(Arc::clone(&post));

        for plane in &served.planes {
            let driver = Arc::clone(&plane.driver);
            peers.keep(tokio::spawn(async move { driver.ticks().await }));
            let driver = Arc::clone(&plane.driver);
            peers.keep(tokio::spawn(async move { driver.drives().await }));
        }
        let lease = crate::root::serve::LeaseTick {
            node: Arc::clone(post.node()),
        };
        peers.keep(tokio::spawn(lease.ticks()));
        crate::dist_root_install::install();
        let library_path = crate::root::test_plugins::application_store_library();
        let statement = crate::root::loader::dispatch::rendering_of_library(&library_path)
            .expect("the actual store statement reads")
            .expect("the actual store exports its statement");
        let module = busbar_contract::abi::mechanism::rendering::read(&statement)
            .expect("the actual store statement is valid")
            .name;
        let mut manifest = crate::root::test_plugins::manifest(
            "store",
            &module,
            crate::root::test_plugins::FIXTURE_PUBLISHER,
        );
        manifest.statement = Some(hex::encode(statement));
        let library = std::fs::read(library_path).expect("the actual store library reads");
        let directory = crate::root::test_plugins::scratch("serve-session-product");
        let archive = crate::root::test_plugins::signed(
            &crate::dist_root_install::release_key(),
            manifest,
            &library,
        );
        std::fs::write(directory.join("store.tar.gz"), archive)
            .expect("stage the actual store archive");
        if !opts.hooks.is_empty() {
            let path = std::path::PathBuf::from(
                std::env::var_os("BUSBAR_TEST_WEBREQUEST_CDYLIB")
                    .expect("BUSBAR_TEST_WEBREQUEST_CDYLIB names the actual built hook"),
            );
            let statement = crate::root::loader::dispatch::rendering_of_library(&path)
                .expect("the actual hook statement reads")
                .expect("the actual hook exports its statement");
            let module = busbar_contract::abi::mechanism::rendering::read(&statement)
                .expect("the actual hook statement is valid")
                .name;
            let mut manifest = crate::root::test_plugins::manifest(
                "hook",
                &module,
                crate::root::test_plugins::FIXTURE_PUBLISHER,
            );
            manifest.statement = Some(hex::encode(statement));
            manifest.needs.prompt = busbar_contract::plugin_rows::NeedLevel::Rw;
            manifest.needs.user = busbar_contract::plugin_rows::NeedLevel::Ro;
            let library = std::fs::read(path).expect("the actual hook library reads");
            let archive = crate::root::test_plugins::signed(
                &crate::dist_root_install::release_key(),
                manifest,
                &library,
            );
            std::fs::write(directory.join("hook.tar.gz"), archive)
                .expect("stage the actual webrequest hook");
        }
        let settings = crate::root::test_plugins::application_store_settings(&directory);
        let signing_file = directory.join("signing.key");
        std::fs::write(&signing_file, [7u8; 32]).expect("the actual token signing secret");
        let hooks: BTreeMap<_, _> = opts
            .hooks
            .iter()
            .map(|(name, hook)| {
                (
                    *name,
                    serde_json::json!({
                        "module": hook.plugin,
                        "settings": hook.settings.authoring(&busbar_kernel::seal::acquire()),
                        "groups": hook.groups,
                        "phase": hook.resolved_stages(),
                        "kind": hook.kind,
                        "timeout_ms": hook.timeout_ms,
                        "on_error": hook.on_error,
                        "on_empty": hook.on_empty,
                        "prompt": hook.prompt,
                        "user": hook.user,
                        "priority": hook.priority,
                    }),
                )
            })
            .collect();
        let mut document = serde_json::json!({
            "providers": {},
            "models": {},
            "groups": groups,
            "hooks": hooks,
            "pools": {
                "hooks": opts.hooks.iter()
                    .filter(|(_, hook)| hook.global)
                    .map(|(name, _)| *name)
                    .collect::<Vec<_>>()
            },
            "auth": { "chain": ["keys"], "admin_auth": [], "signing_key": {"file": signing_file} },
            "store": { "module": module, "settings": settings },
            "advanced": { "allow_destinations": ["127.0.0.1"] },
        });
        if let Some(guarded) = opts.oauth_admin {
            document["oauth_as"] = serde_json::json!({
                "issuer": "http://127.0.0.1:9999", "default_grant": ["read"],
            });
            if guarded {
                let operator = busbar_kernel::config::operator_provider();
                let credential = directory.join("consent-operator");
                std::fs::write(&credential, "consent-operator-credential").unwrap();
                document["identity-providers"] = serde_json::json!({
                    (operator): {"module": operator, "token": {"file": credential}},
                });
                document["auth"]["admin_auth"] = serde_json::json!([operator]);
            }
        }
        let deploy = busbar_kernel::config::deploy_from_deserializer(document)
            .expect("the session deployment parses");
        let mut config = busbar_kernel::config::resolve(&deploy, &Default::default())
            .expect("the session deployment resolves");
        if budget.is_some() {
            config.plane_fees.insert(
                plane_name.clone(),
                busbar_kernel_ledger::cost::PlaneFees {
                    per_request: 0,
                    per_session: 1,
                },
            );
        }
        let data_chain_names: Vec<_> = config
            .auth
            .as_ref()
            .into_iter()
            .flat_map(|auth| auth.chain.iter().map(|entry| entry.name.clone()))
            .collect();
        let plugins = crate::root::test_plugins::trusted_config(&directory);
        let rules = std::sync::Arc::new(crate::root::door_steps::Rules::default())
            as std::sync::Arc<dyn busbar_kernel::appbuild::RootRules>;
        let (mut app, rotate, limits) = crate::root::serve::door_tests::build_process_app(
            config,
            plugins,
            None,
            Default::default(),
            Default::default(),
            (None, None),
            None,
            &rules,
        )
        .await
        .expect("the actual product builds the session deployment");
        limits.stage_boot();
        crate::root::serve::door_tests::attach_process_prices(&app)
            .await
            .expect("native process price history");
        limits.keep().await.expect("the product limits commit");
        if let Some(rotate) = rotate {
            rotate.await;
        }
        if budget == Some(0) {
            let (warm, _) = gov
                .mint_signed_async(
                    NewKeySpec {
                        name: "pre-exhausted-session-group".to_string(),
                        group: groups.keys().next().cloned(),
                        ..Default::default()
                    },
                    4_000_000_000,
                    1_700_000_000,
                )
                .await
                .expect("the prior caller shares the actual configured session group");
            let pool = crate::root::door_steps::plane_lane(&plane_name, "m-cap");
            drop(
                gov.try_admit(&cost, &warm, &pool, busbar_kernel::store::now())
                    .expect("the prior caller is admitted before opening its session"),
            );

            gov.record_usage(
                &cost,
                &warm,
                &pool,
                &busbar_kernel_ledger::cost::plane_fee_lane(&plane_name),
                &BTreeMap::from([(busbar_kernel_ledger::cost::PER_SESSION.to_owned(), 1)]),
                busbar_kernel::store::now(),
            );
            assert!(
                gov.try_admit(&cost, &warm, &pool, busbar_kernel::store::now())
                    .is_err(),
                "the actual session group is exhausted before the tested request"
            );
        }
        app.amendments = Arc::clone(gov.amendments());
        app.governance = Some(Arc::clone(&gov));
        app.cost = Arc::new(cost);
        app.serve_tables = tables.clone();
        app.plugin_routes = super::metrics_routes();
        let app = std::sync::Arc::new(app);
        busbar_kernel::boot::hydrate_all(&app).await.expect(
            "normal startup restores the session deployment's native audit owner before serving",
        );

        let upgrades = crate::root::serve::upgrade_carriers(crate::LINKED.transports, &[]);
        let mut offers = crate::root::serve::transport_offers(crate::LINKED.transports, &[]);

        let physical_scheme = crate::root::listener::framed::DATA_SCHEME;
        let physical = connector
            .framer_for(physical_scheme)
            .expect("the actual HTTP framer");
        let physical_facts = physical.facts();
        assert_eq!(
            physical_facts.role,
            busbar_contract::abi::transport::ROLE_FRAMER
        );
        assert!(!physical_facts.datagram && physical.accepts());
        assert!(physical_facts.claims.contains(&physical_scheme));
        let physical_row = crate::LINKED
            .transports
            .iter()
            .find(|row| (row.key)() == physical_facts.name)
            .expect("the loaded physical framer has its actual linked Statement");
        assert_eq!((physical_row.claims)(), physical_facts.claims);
        let physical_points = (physical_row.offered)();
        let mut physical_carriers = std::collections::BTreeSet::new();
        for plane in &served.planes {
            for claim in plane
                .snapshot
                .claims
                .iter()
                .filter(|claim| crate::root::serve::routed(claim))
            {
                assert!(
                    claim.carrier == physical_scheme
                        || claim.carrier == "ws"
                        || claim.carrier == "webrtc",
                    "this fixture places only HTTP requests and their session upgrades"
                );
                assert!(claim.verb == "GET" || claim.verb == "POST");
                assert!(offers.iter().any(|(scheme, _)| *scheme == claim.carrier));
                physical_carriers.insert(claim.carrier.clone());
            }
        }
        for (scheme, points) in &mut offers {
            if physical_carriers.contains(*scheme) {
                *points = physical_points;
            }
        }

        let data_chain: Vec<_> = data_chain_names
            .iter()
            .map(|name| (name.clone(), app.auth.position_points(name)))
            .collect();
        let core = opts
            .oauth_admin
            .map(|_| busbar_kernel::base_data_core_lines(&app))
            .unwrap_or_default();
        let mounts = crate::root::serve::door_mounts(
            served,
            || CARD.pin(),
            &data_chain,
            &core,
            &upgrades,
            &offers,
        )
        .expect("its claims mount");
        let data = Arc::clone(&mounts.data);
        let (router, _admin, handle) = busbar_kernel::build_split_routers_serving_doors(
            Arc::clone(&app),
            mounts.routes,
            mounts.sessions,
            mounts.refusals,
            1 << 20,
            0,
            false,
        );
        live_app
            .set(Arc::downgrade(&handle))
            .expect("the session handle installs once");
        services
            .kernel()
            .expect("the actual kernel is installed")
            .attach_tickets(Arc::new(crate::root::serve::LiveTickets(Arc::downgrade(
                &handle,
            ))));
        crate::root::inbound::publish(&app).expect("the actual session claims publish");
        let inbound_connector = Arc::clone(&connector);
        let framer = crate::root::listener::framed::data_framer(Arc::new(move |scheme| {
            inbound_connector.framer_for(scheme)
        }))
        .expect("the actual HTTP and WebSocket framers are linked")
        .with_dispatch(Arc::new(move |_| Some(Arc::clone(&data))));
        Some(Ok(SessionDoor {
            framer,
            connector,
            media: MediaStop(media_stop),
            peers,
            media_messages: tokio::sync::Mutex::new(Vec::new()),
            _handle: handle,
            router,
            heard,
            gov,
            app,
            key,
            plain: plain.expose(&busbar_kernel::seal::lend()).clone(),
            live,
            money,
            post,
            book,
        }))
    }

    async fn serve_listener(
        listener: tokio::net::TcpListener,
        router: axum::Router,
        framer: crate::root::listener::framed::DataFramer,
    ) {
        let _ = crate::root::listener::serve_admitted_framed(
            crate::root::listener::SocketAdmits::new(listener),
            router,
            None,
            std::future::pending::<()>(),
            None,
            Some(framer),
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn physical_core_consent_uses_admin_auth_without_opening_data_auth() {
        const TEST: &str = concat!(
            module_path!(),
            "::physical_core_consent_uses_admin_auth_without_opening_data_auth"
        );
        let Some(case) = super::independent_cases(TEST, &["open", "operator"]) else {
            return;
        };
        let _one = PUBLISHING.lock().await;
        let s = session_with(
            "core-consent-auth",
            Opts {
                oauth_admin: Some(case == "operator"),
                ..Opts::default()
            },
        )
        .await
        .expect("the distribution links the actual physical session fixture");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(serve_listener(listener, s.router.clone(), s.framer.clone()));
        let client = reqwest::Client::builder()
            .timeout(ANSWER_BOUND)
            .build()
            .unwrap();
        let registered = client
            .post(format!("{origin}/register"))
            .json(
                &serde_json::json!({"redirect_uris":["http://127.0.0.1:9998/callback"],
                "token_endpoint_auth_method":"none", "response_types":["code"],
                "grant_types":["authorization_code"], "scope":"read"}),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(registered.status(), 201);
        let client_id = registered.json::<serde_json::Value>().await.unwrap()["client_id"]
            .as_str()
            .unwrap()
            .to_string();
        let consent = reqwest::Url::parse_with_params(
            &format!("{origin}/consent"),
            &[(
                "return",
                format!("/authorize?client_id={client_id}&scope=read"),
            )],
        )
        .unwrap();
        let absent = if case == "open" { 200 } else { 401 };
        let stats = format!("{origin}/stats");
        for (url, token, expected) in [
            (consent.as_str(), None, absent),
            (consent.as_str(), Some("incorrect-operator"), absent),
            (consent.as_str(), Some("consent-operator-credential"), 200),
            (stats.as_str(), None, 401),
            (stats.as_str(), Some("incorrect-operator"), 401),
            (stats.as_str(), Some("consent-operator-credential"), 401),
        ] {
            let mut request = client.get(url);
            if let Some(token) = token {
                request = request.bearer_auth(token);
            }
            let response = request.send().await.unwrap();
            assert_eq!(response.status(), expected, "{case}: {url}");
            let body = response.text().await.unwrap();
            assert_eq!(
                body.contains("id=approve") && body.contains("id=deny"),
                expected == 200
            );
        }
        server.abort();
        let _ = server.await;
        super::independent_case_done(TEST, &case);
    }

    impl SessionDoor {
        pub(in crate::root::serve) async fn mint(&self) -> axum::response::Response {
            send_body(&self.router, MINT, Some(&self.plain), "{}").await
        }

        async fn requests(&self) -> u64 {
            self.gov
                .usage_for_async(&self.app.cost, &self.key.id, busbar_kernel::store::now())
                .await
                .expect("a read")
                .expect("the key exists")
                .requests
        }

        fn lines(&self) -> usize {
            let records = self
                .book
                .durability
                .lock()
                .expect("unpoisoned")
                .audit_records
                .clone();
            records
                .iter()
                .filter(|r| {
                    r.outcome.unit_end.is_completed()
                        && r.what
                            .destination
                            .as_deref()
                            .is_some_and(|lane| lane.ends_with("m-cap"))
                })
                .count()
        }

        async fn scrape(&self) -> String {
            use tower::ServiceExt as _;
            let req = axum::http::Request::builder()
                .method("GET")
                .uri("/metrics")
                .header("authorization", format!("Bearer {}", self.plain))
                .body(axum::body::Body::empty())
                .expect("a request");
            let response = self
                .router
                .clone()
                .oneshot(req)
                .await
                .expect("the router answers");
            assert_eq!(response.status(), StatusCode::OK, "the scrape is served");
            let body = axum::body::to_bytes(response.into_body(), 1 << 22)
                .await
                .expect("the exposition");
            String::from_utf8(body.to_vec()).expect("the exposition is UTF-8")
        }

        fn audit_records(&self) -> Vec<busbar_kernel_audit::AuditRecord> {
            self.book
                .durability
                .lock()
                .expect("unpoisoned")
                .audit_records
                .clone()
        }

        async fn local_ticket(&mut self) -> String {
            let before = busbar_kernel::store::now();
            let response = self.mint().await;
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "the local ticket is issued"
            );
            let body: serde_json::Value = serde_json::from_slice(&body_of(response).await)
                .expect("the local ticket response is JSON");
            let ticket = body["value"]
                .as_str()
                .expect("the issued ticket")
                .to_owned();
            let expires = body["expires_at_unix"].as_u64().expect("its expiry");
            assert!(
                ticket.starts_with("ek_"),
                "a browser ticket, never the real provider key"
            );
            assert_ne!(ticket, CREDENTIAL);
            assert_ne!(ticket, self.plain);
            assert!(
                expires > before && expires <= busbar_kernel::store::now() + 600,
                "a short-lived browser ticket: {expires}"
            );
            assert!(
                self.heard.try_recv().is_err(),
                "local issuance never dials a provider"
            );
            ticket
        }

        async fn signal(&self, token: &str) -> (StatusCode, String) {
            self.signal_inner(token, true).await
        }

        async fn signal_inner(&self, token: &str, wait_for_channel: bool) -> (StatusCode, String) {
            let (mut peer, offer) = RtcPeer::offer(
                self.connector
                    .framer_for("webrtc")
                    .expect("the actual browser WebRTC framer"),
            )
            .await;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("a signal listener");
            let address = listener.local_addr().expect("its address");
            let serving = tokio::spawn(serve_listener(
                listener,
                self.router.clone(),
                self.framer.clone(),
            ));
            let exchange = async {
                let socket = tokio::net::TcpStream::connect(address)
                    .await
                    .expect("the signal connection");
                let (mut sender, connection) =
                    hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(socket))
                        .await
                        .expect("the signal HTTP client");
                self.peers.keep(tokio::spawn(async move {
                    connection.await.expect("the signal HTTP connection");
                }));
                let request = axum::http::Request::builder()
                    .method("POST")
                    .uri(SIGNAL)
                    .header("host", address.to_string())
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/sdp")
                    .header("connection", "close")
                    .body(axum::body::Body::from(offer))
                    .expect("the actual peer SDP request");
                let response = sender
                    .send_request(request)
                    .await
                    .expect("the signal response");
                let status = response.status();
                let mut head = format!("HTTP/1.1 {status}\r\n");
                for (name, value) in response.headers() {
                    head.push_str(&format!(
                        "{name}: {}\r\n",
                        value.to_str().expect("HTTP response field")
                    ));
                }
                head.push_str("\r\n");
                let body =
                    axum::body::to_bytes(axum::body::Body::new(response.into_body()), 1 << 20)
                        .await
                        .expect("the decoded signal response body")
                        .to_vec();
                if !status.is_success() {
                    eprintln!(
                        "signal refusal {status}: {}",
                        String::from_utf8_lossy(&body)
                    );
                    if status == StatusCode::UNAUTHORIZED {
                        let identity = self
                            .gov
                            .prepare_ticket_redemption_async(
                                "ek_",
                                token,
                                busbar_kernel::store::now(),
                            )
                            .await;
                        eprintln!(
                            "signal ticket native verification: {}; issuance receipts: {}",
                            identity.is_some(),
                            self.audit_records().len()
                        );
                    }
                }
                (status, head, body)
            };
            let result = tokio::time::timeout(ANSWER_BOUND, exchange).await;
            self.peers.keep(serving);
            let (status, head, body) = result.expect("the signal answers within its bound");
            if status.is_success() {
                assert!(
                    head.lines()
                        .any(|line| line.eq_ignore_ascii_case("content-type: application/sdp")),
                    "the actual media signal returns SDP"
                );
                peer.answer(&body);
                let (task, received, opened) = peer.run_ready(self.media.0.subscribe(), None);
                self.peers.keep(task);
                self.media_messages.lock().await.push(MediaPeer {
                    ready: false,
                    received,
                });
                if wait_for_channel {
                    tokio::time::timeout(ANSWER_BOUND, opened)
                        .await
                        .expect("the actual browser peer establishes its data channel")
                        .expect("the browser peer stays alive through its opening");
                }
            }
            (status, head)
        }

        async fn wait_media_ready(&self) {
            let mut all = self.media_messages.lock().await;
            let MediaPeer { ready, received } = all
                .last_mut()
                .expect("an actual browser media peer is retained");
            if *ready {
                return;
            }
            tokio::time::timeout(ANSWER_BOUND, async {
                while let Some(bytes) = received.recv().await {
                    if serde_json::from_slice::<serde_json::Value>(&bytes)
                        .ok()
                        .and_then(|value| {
                            value
                                .get("type")
                                .and_then(|kind| kind.as_str())
                                .map(str::to_owned)
                        })
                        .as_deref()
                        == Some("session.created")
                    {
                        return;
                    }
                }
                panic!("the actual media peer ended before the provider's opening event");
            })
            .await
            .expect("actual ICE, DTLS, SCTP and provider media establish");
            *ready = true;
        }

        async fn finish_media(&self) {
            self.wait_media_ready().await;
            let _ = self.media.0.send(true);
            tokio::time::timeout(ANSWER_BOUND, async {
                while self.money.open_units() != 0 || self.post.open_units() != 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("the actual media shutdown settles its one lifecycle");
        }

        async fn signal_once(&mut self) -> (bool, bool) {
            let ticket = self.local_ticket().await;
            let admitted = self.requests().await;
            let after_mint = self.audit_records().last().map_or(0, |record| record.seq);
            let (status, _) = self.signal_inner(&ticket, false).await;
            let failed = if status.is_success() {
                tokio::time::timeout(ANSWER_BOUND, async {
                    loop {
                        if self.money.open_units() == 0 && self.post.open_units() == 0 {
                            if let Some(record) = self
                                .audit_records()
                                .into_iter()
                                .find(|record| record.seq > after_mint)
                            {
                                return !record.outcome.unit_end.is_completed()
                                    || record.outcome.finish
                                        == busbar_kernel_audit::FinishClass::Error;
                            }
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                })
                .await
                .expect("the actual upstream failure terminates the admitted session")
            } else {
                assert!(
                    status.is_server_error(),
                    "a failed provider or tripped route is a server failure"
                );
                true
            };
            assert_eq!(
                self.requests().await,
                admitted,
                "consuming the ticket never adds a second admission"
            );
            (failed, self.heard.try_recv().is_ok())
        }

        async fn mint_once(&mut self) -> (StatusCode, bool) {
            let response = self.mint().await;
            let status = response.status();
            let _ = body_of(response).await;

            (status, self.heard.try_recv().is_ok())
        }
    }

    fn counted(exposition: &str, family: &str) -> f64 {
        exposition
            .lines()
            .filter(|l| l.starts_with(&format!("{family}{{")))
            .filter(|l| {
                l.split("lane=\"")
                    .nth(1)
                    .and_then(|rest| rest.split('"').next())
                    .is_some_and(|lane| lane.ends_with("m-cap"))
            })
            .filter_map(|l| l.rsplit_once(' ')?.1.parse::<f64>().ok())
            .sum()
    }

    const UNAVAILABLE: &str = r#"{"error":{"message":"overloaded","type":"server_error"}}"#;

    const MAX_FAILURES: usize = 8;

    async fn trip(s: &mut SessionDoor) -> Option<usize> {
        for n in 0..=MAX_FAILURES {
            let (failed, heard) = s.signal_once().await;
            assert!(failed, "the actual upstream session {n} is not served");
            if !heard {
                assert!(
                    n > 0,
                    "the breaker can only open after a real failed provider attempt"
                );
                return Some(n);
            }
        }
        None
    }

    fn one_an_hour() -> busbar_kernel::config::groups::LimitCfg {
        busbar_kernel::config::groups::LimitCfg {
            metric: busbar_kernel::config::groups::LimitMetric::Requests,
            amount: 1,
            per: Some(busbar_kernel::config::groups::LimitWindow::Hour),
            scope: None,
            on_exhaust: None,
            downgrade_to: None,
            admission: None,
            on_exhaustion: None,
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_session_door_mint_is_served_under_the_providers_credential() {
        let _one = PUBLISHING.lock().await;
        let mut s = session_served("serve-door-session", None)
            .await
            .expect("the build links the session door");

        let refused = send_body(&s.router, MINT, None, "{}").await;
        assert_eq!(
            refused.status().as_u16(),
            u16::try_from(refusal_status(ReasonCode::Unauthenticated)).expect("a status"),
            "an unkeyed caller is refused at the session door"
        );
        assert!(s.heard.try_recv().is_err(), "nothing was dialled for it");
        assert_eq!(s.requests().await, 0, "nor charged");

        let ticket = s.local_ticket().await;
        assert_eq!(s.requests().await, 1, "local issuance is the one admission");
        let issuance = s.audit_records();
        assert_eq!(
            issuance.len(),
            1,
            "issuance sealed before its ticket is usable"
        );
        assert_eq!(
            issuance[0].usage.fee_count, 0,
            "local issuance reports no session fee"
        );
        let (status, _response) = s.signal(&ticket).await;
        assert_eq!(status.as_u16(), 201, "the actual ticket opens its SDP call");

        let head = tokio::time::timeout(ANSWER_BOUND, s.heard.recv())
            .await
            .expect("the provider's dial is heard within the bound")
            .expect("the provider was dialled");
        assert!(head.starts_with(&format!("POST {SIGNAL} ")), "{head}");
        assert!(
            head.lines()
                .any(|l| l.eq_ignore_ascii_case(&format!("authorization: Bearer {CREDENTIAL}"))),
            "the provider's own credential: {head}"
        );
        assert!(
            !head.contains(&s.plain),
            "the caller's token never reaches the provider: {head}"
        );
        assert!(
            !head.contains(&ticket),
            "the browser ticket never reaches the provider: {head}"
        );
        assert!(
            head.contains(r#""model":"m-cap""#),
            "the locked session on the catalog model: {head}"
        );

        s.finish_media().await;
        assert_eq!(
            s.requests().await,
            1,
            "one admitted request, on the presenting key"
        );
        assert_eq!(s.money.open_units(), 0, "its money facts closed");
        assert_eq!(s.post.open_units(), 0, "its node facts closed");
        let records = s.audit_records();
        assert_eq!(
            records.len(),
            issuance.len() + 1,
            "one new media terminal receipt"
        );
        assert!(
            records[0] == issuance[0],
            "the issuance receipt is unchanged"
        );
        let (mint, media) = (&records[0], &records[1]);
        assert_ne!(
            mint.what.unit_key, media.what.unit_key,
            "distinct physical units"
        );
        assert_eq!(media.seq, mint.seq + 1, "contiguous phase receipts");
        assert!(
            media.prev_hash == mint.hash,
            "the media receipt follows issuance"
        );
        assert!(
            mint.what.op_class == media.what.op_class,
            "one session operation class"
        );
        assert!(
            mint.what.destination == media.what.destination,
            "one verified route"
        );
        for record in &records {
            assert!(
                record.subject == busbar_kernel_audit::Subject::PrincipalId(s.key.id.clone()),
                "both phases belong to the presenting key"
            );
            assert!(
                record.what.parent.is_none(),
                "each phase is an external request"
            );
            assert!(
                record.outcome.unit_end.is_completed(),
                "both phases completed"
            );
            assert!(
                record
                    .what
                    .destination
                    .as_deref()
                    .is_some_and(|lane| lane.ends_with("m-cap")),
                "both receipts retain the locked model"
            );
            assert!(
                record.usage.lines.is_empty(),
                "this session reported no usage quantities"
            );
        }
        assert_eq!(
            media.usage.fee_count, 1,
            "the actual session reports its fee once"
        );
        assert_eq!(
            records
                .iter()
                .map(|record| record.usage.fee_count)
                .sum::<u32>(),
            1,
            "one fee across both phase receipts"
        );
        assert!(busbar_kernel_audit::AuditChain::verify_chain(&records).is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_spent_key_is_refused_at_the_session_door_before_any_dial() {
        let _one = PUBLISHING.lock().await;
        let mut s = session_served("serve-door-session-spent", Some(0))
            .await
            .expect("the build links the session door");
        let response = send_body(&s.router, MINT, Some(&s.plain), "{}").await;
        assert_eq!(
            response.status().as_u16(),
            429,
            "an over-budget caller is refused at admission"
        );
        assert!(
            s.heard.try_recv().is_err(),
            "the provider was never dialled"
        );
        assert_eq!(s.requests().await, 0, "the refusal charged nothing");
        assert_eq!(s.money.open_units(), 0);
        assert_eq!(s.post.open_units(), 0);
        assert_eq!(s.lines(), 0, "no record carries counts");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_failing_provider_opens_the_breaker_cell_of_the_doors_route() {
        let _one = PUBLISHING.lock().await;
        let mut s = session_with(
            "serve-door-session-trip",
            Opts {
                far: Some(Arc::new(|_| {
                    Far::Says("503 Service Unavailable", UNAVAILABLE)
                })),
                ..Opts::default()
            },
        )
        .await
        .expect("the build links the session door");
        let failures = counted(
            &s.scrape().await,
            busbar_kernel::telemetry::UPSTREAM_FAILURES_TOTAL,
        );

        let dialled = trip(&mut s)
            .await
            .unwrap_or_else(|| panic!("{MAX_FAILURES} failing signals never opened the cell"));
        assert!(dialled >= 1, "the cell opened on a failure it recorded");
        assert!(
            counted(
                &s.scrape().await,
                busbar_kernel::telemetry::UPSTREAM_FAILURES_TOTAL,
            ) >= failures + 1.0,
            "the provider's failures are counted under the route's lane"
        );
        assert_eq!(s.money.open_units(), 0, "every unit's money facts closed");
        assert_eq!(s.post.open_units(), 0, "every unit's node facts closed");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_tripped_provider_is_refused_at_the_session_door_before_the_dial_at_once() {
        let _one = PUBLISHING.lock().await;
        let holding = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let held = Arc::clone(&holding);
        let mut s = session_with(
            "serve-door-session-fastfail",
            Opts {
                far: Some(Arc::new(move |_| {
                    if held.load(std::sync::atomic::Ordering::SeqCst) {
                        Far::Holds
                    } else {
                        Far::Says("503 Service Unavailable", UNAVAILABLE)
                    }
                })),
                ..Opts::default()
            },
        )
        .await
        .expect("the build links the session door");
        trip(&mut s)
            .await
            .unwrap_or_else(|| panic!("{MAX_FAILURES} failing signals never opened the cell"));

        holding.store(true, std::sync::atomic::Ordering::SeqCst);
        let started = std::time::Instant::now();
        let (failed, heard) =
            tokio::time::timeout(std::time::Duration::from_secs(5), s.signal_once())
                .await
                .expect("the tripped route's signal is answered, not held at its provider");
        let took = started.elapsed();
        assert!(failed, "the actual tripped session is refused");
        assert!(!heard, "the refused signal never reached the provider");
        assert!(
            took < std::time::Duration::from_secs(1),
            "refused before the dial, in milliseconds: {took:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_gate_hook_refuses_the_mint_before_the_provider_is_dialled() {
        let _one = PUBLISHING.lock().await;
        let armed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut s = session_with(
            "serve-door-session-gated",
            Opts {
                hooks: vec![(
                    "closed",
                    global_gate(
                        serde_json::json!({
                            "reject": {"status": 409, "message": "no new sessions"}
                        }),
                        Arc::clone(&armed),
                    )
                    .await,
                )],
                ..Opts::default()
            },
        )
        .await
        .expect("the build links the session door");
        let ticket = s.local_ticket().await;
        let admitted = s.requests().await;
        let completed_before_signal = s.lines();
        armed.store(true, std::sync::atomic::Ordering::SeqCst);
        let (status, _) = s.signal(&ticket).await;
        let heard = s.heard.try_recv().is_ok();
        assert_eq!(
            s.requests().await,
            admitted,
            "a gated signal adds no second admission"
        );
        assert_eq!(status.as_u16(), 409, "the hook's own status");
        assert!(!heard, "the gated signal never reached the provider");
        assert_eq!(
            s.money.open_units(),
            0,
            "the refused unit's money facts closed"
        );
        assert_eq!(
            s.post.open_units(),
            0,
            "the refused unit's node facts closed"
        );
        assert_eq!(
            s.lines(),
            completed_before_signal,
            "the gated signal adds no completed record carrying counts"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn every_session_door_mint_served_or_refused_seals_one_record_on_one_chain() {
        let _one = PUBLISHING.lock().await;
        let mut s = session_with(
            "serve-door-session-audit",
            Opts {
                limits: vec![one_an_hour()],
                ..Opts::default()
            },
        )
        .await
        .expect("the build links the session door");
        let ticket = s.local_ticket().await;
        assert_eq!(
            s.requests().await,
            1,
            "the local ticket is the one admission"
        );
        let answer_bound = tokio::time::Instant::now() + ANSWER_BOUND;
        let (status, _signal) = s.signal(&ticket).await;
        assert_eq!(status.as_u16(), 201, "the admitted ticket places its call");
        tokio::time::timeout_at(answer_bound, s.wait_media_ready())
            .await
            .expect("the actual caller and provider establish within the original answer bound");

        tokio::time::timeout_at(answer_bound, async {
            while s.heard.is_empty() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the signal reaches its actual provider within the answer bound");
        assert!(
            s.heard.try_recv().is_ok(),
            "the signal reached its provider"
        );
        assert_eq!(
            s.requests().await,
            1,
            "ticket consumption never charges a second admission"
        );
        s.finish_media().await;
        let (status, heard) = s.mint_once().await;
        assert_ne!(status, StatusCode::OK, "the second is refused by the limit");
        assert!(!heard, "before its dial");

        let records = s.audit_records();
        assert_eq!(
            records.len(),
            3,
            "one record for local issuance, its signal, and the refused issuance: {records:?}"
        );
        let (served, signal, refused) = (&records[0], &records[1], &records[2]);
        for record in &records {
            assert_eq!(
                record.subject,
                busbar_kernel_audit::Subject::PrincipalId(s.key.id.clone()),
                "one presenting/billing key across issuance and consumption"
            );
        }
        assert_eq!(
            signal.outcome.unit_end,
            busbar_contract::caps::Outcome::Completed
        );
        assert_eq!(signal.seq, served.seq + 1);
        assert_eq!(signal.prev_hash, served.hash);
        assert_eq!(
            served.what.op_class.as_str(),
            refused.what.op_class.as_str(),
            "both sealed under the door's one operation class"
        );
        assert_eq!(
            served.outcome.unit_end,
            busbar_contract::caps::Outcome::Completed
        );
        assert!(
            matches!(
                refused.outcome.unit_end,
                busbar_contract::caps::Outcome::Refused(_, ReasonCode::RateLimited)
            ),
            "the refusal is chained as one: {:?}",
            refused.outcome
        );
        assert_eq!(refused.seq, signal.seq + 1, "contiguous");
        assert_eq!(
            refused.prev_hash, signal.hash,
            "linked to the record before"
        );
        assert!(busbar_kernel_audit::AuditChain::verify_window(&records).is_ok());
        assert!(
            s.book
                .durability
                .lock()
                .expect("unpoisoned")
                .retained_audit_findings()
                .is_empty(),
            "the node's own verify finds nothing"
        );
        let mut forged = records.clone();
        forged[0].outcome.unit_end = refused.outcome.unit_end;
        assert!(
            busbar_kernel_audit::AuditChain::verify_window(&forged).is_err(),
            "a record altered after its seal breaks the chain"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_served_mint_counts_its_upstream_attempt_under_the_doors_route() {
        let _one = PUBLISHING.lock().await;
        let mut s = session_served("serve-door-session-metrics", None)
            .await
            .expect("the build links the session door");
        let before = counted(
            &s.scrape().await,
            busbar_kernel::telemetry::UPSTREAM_ATTEMPTS_TOTAL,
        );
        let ticket = s.local_ticket().await;
        let admitted = s.requests().await;
        let (status, _) = s.signal(&ticket).await;
        assert_eq!(status.as_u16(), 201, "the actual signal is served");
        let heard = tokio::time::timeout(ANSWER_BOUND, s.heard.recv())
            .await
            .expect("the actual upstream attempt is heard")
            .is_some();
        s.finish_media().await;
        assert_eq!(
            s.requests().await,
            admitted,
            "ticket consumption adds no second admission"
        );
        assert!(heard, "dialled");
        let exposition = s.scrape().await;
        assert_eq!(
            counted(
                &exposition,
                busbar_kernel::telemetry::UPSTREAM_ATTEMPTS_TOTAL
            ),
            before + 1.0,
            "one attempt, counted under the route's lane:\n{exposition}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_key_short_of_the_doors_grant_is_refused_before_any_dial() {
        let _one = PUBLISHING.lock().await;
        let mut s = session_with(
            "serve-door-session-ungranted",
            Opts {
                allowed_pools: Some(Vec::new()),
                ..Opts::default()
            },
        )
        .await
        .expect("the build links the session door");
        let (status, heard) = s.mint_once().await;
        assert!(
            status.is_client_error(),
            "a key outside the door's grant is refused: {status}"
        );
        assert!(!heard, "the provider was never dialled");
        assert_eq!(s.requests().await, 0, "the refusal charged nothing");
        assert_eq!(s.money.open_units(), 0);
        assert_eq!(s.post.open_units(), 0);
        assert_eq!(s.lines(), 0, "no record carries counts");
        let (signal, response) = s.signal(&s.plain).await;
        assert!(
            signal.is_client_error(),
            "the same denied grant cannot open a signal: {response}"
        );
        assert!(
            s.heard.try_recv().is_err(),
            "nor can the denied signal dial"
        );
        assert_eq!(
            s.requests().await,
            0,
            "neither denied entry charges the caller"
        );
        assert_eq!(s.money.open_units(), 0);
        assert_eq!(s.post.open_units(), 0);
    }

    fn sha1(data: &[u8]) -> [u8; 20] {
        let mut h: [u32; 5] = [
            0x6745_2301,
            0xEFCD_AB89,
            0x98BA_DCFE,
            0x1032_5476,
            0xC3D2_E1F0,
        ];
        let mut msg = data.to_vec();
        let bits = (data.len() as u64).wrapping_mul(8);
        msg.push(0x80);
        while msg.len() % 64 != 56 {
            msg.push(0);
        }
        msg.extend_from_slice(&bits.to_be_bytes());
        for block in msg.chunks(64) {
            let mut w: Vec<u32> = block
                .chunks(4)
                .map(|word| u32::from_be_bytes([word[0], word[1], word[2], word[3]]))
                .collect();
            while w.len() < 80 {
                let n = w.len();
                w.push((w[n - 3] ^ w[n - 8] ^ w[n - 14] ^ w[n - 16]).rotate_left(1));
            }
            let [mut a, mut b, mut c, mut d, mut e] = h;
            for (i, wi) in w.iter().enumerate() {
                let (f, k) = match i {
                    0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                    20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                    40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                    _ => (b ^ c ^ d, 0xCA62_C1D6),
                };
                let t = a
                    .rotate_left(5)
                    .wrapping_add(f)
                    .wrapping_add(e)
                    .wrapping_add(k)
                    .wrapping_add(*wi);
                e = d;
                d = c;
                c = b.rotate_left(30);
                b = a;
                a = t;
            }
            for (slot, v) in h.iter_mut().zip([a, b, c, d, e]) {
                *slot = slot.wrapping_add(v);
            }
        }
        let mut out = [0u8; 20];
        for (i, v) in h.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
        }
        out
    }

    /// A fresh RFC 6455 `Sec-WebSocket-Key`: base64 of 16 random bytes, minted per open (no fixed
    /// nonce lives in source; the door answers whatever key the caller offered).
    fn ws_nonce() -> String {
        use base64::Engine as _;
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).expect("the OS CSPRNG is available");
        base64::engine::general_purpose::STANDARD.encode(nonce)
    }

    fn ws_accept(key: &str) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(sha1(
            format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11").as_bytes(),
        ))
    }

    async fn read_head(socket: &mut tokio::net::TcpStream) -> Option<String> {
        let mut got = Vec::new();
        let mut byte = [0u8; 1];
        while !got.ends_with(b"\r\n\r\n") {
            match socket.read(&mut byte).await {
                Ok(1) => got.push(byte[0]),
                _ => return None,
            }
        }
        Some(String::from_utf8_lossy(&got).into_owned())
    }

    async fn read_frame(socket: &mut tokio::net::TcpStream) -> Option<(u8, Vec<u8>)> {
        let mut head = [0u8; 2];
        socket.read_exact(&mut head).await.ok()?;
        let opcode = head[0] & 0x0F;
        let masked = head[1] & 0x80 != 0;
        let len = match head[1] & 0x7F {
            126 => {
                let mut n = [0u8; 2];
                socket.read_exact(&mut n).await.ok()?;
                u64::from(u16::from_be_bytes(n))
            }
            127 => {
                let mut n = [0u8; 8];
                socket.read_exact(&mut n).await.ok()?;
                u64::from_be_bytes(n)
            }
            n => u64::from(n),
        };
        let mut mask = [0u8; 4];
        if masked {
            socket.read_exact(&mut mask).await.ok()?;
        }
        let mut payload = vec![0u8; usize::try_from(len).ok()?];
        socket.read_exact(&mut payload).await.ok()?;
        if masked {
            for (i, b) in payload.iter_mut().enumerate() {
                *b ^= mask[i % 4];
            }
        }
        Some((opcode, payload))
    }

    fn client_text(text: &str) -> Vec<u8> {
        let bytes = text.as_bytes();
        let mask = [0x37u8, 0xFA, 0x21, 0x3D];
        let mut frame = vec![0x81u8];
        match bytes.len() {
            n if n < 126 => frame.push(0x80 | u8::try_from(n).expect("short")),
            n => {
                frame.push(0x80 | 126);
                frame.extend_from_slice(&u16::try_from(n).expect("a short frame").to_be_bytes());
            }
        }
        frame.extend_from_slice(&mask);
        frame.extend(bytes.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        frame
    }

    fn server_text(text: &str) -> Vec<u8> {
        let bytes = text.as_bytes();
        let mut frame = vec![0x81u8, u8::try_from(bytes.len()).expect("a short frame")];
        frame.extend_from_slice(bytes);
        frame
    }

    const FAR_OPENING: &str = r#"{"type":"session.created","session":{"id":"sess_door_0001"}}"#;

    async fn far_socket() -> (u16, tokio::sync::mpsc::UnboundedReceiver<String>) {
        far_socket_opening(Some(FAR_OPENING)).await
    }

    async fn far_socket_silent() -> (u16, tokio::sync::mpsc::UnboundedReceiver<String>) {
        far_socket_opening(None).await
    }

    async fn far_socket_opening(
        opening: Option<&'static str>,
    ) -> (u16, tokio::sync::mpsc::UnboundedReceiver<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port");
        let port = listener.local_addr().expect("its address").port();
        let (sent, heard) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let sent = sent.clone();
                tokio::spawn(async move {
                    let Some(head) = read_head(&mut socket).await else {
                        return;
                    };
                    let Some(key) = head.lines().find_map(|l| {
                        let (name, value) = l.split_once(':')?;
                        name.eq_ignore_ascii_case("sec-websocket-key")
                            .then(|| value.trim().to_string())
                    }) else {
                        return;
                    };
                    let reply = format!(
                        "HTTP/1.1 101 Switching Protocols\r\nupgrade: websocket\r\n\
                         connection: Upgrade\r\nsec-websocket-accept: {}\r\n\r\n",
                        ws_accept(&key)
                    );
                    if socket.write_all(reply.as_bytes()).await.is_err() {
                        return;
                    }
                    let spoke = match opening {
                        Some(opening) => socket.write_all(&server_text(opening)).await.is_ok(),
                        None => true,
                    };
                    if !spoke {
                        return;
                    }
                    while let Some((opcode, payload)) = read_frame(&mut socket).await {
                        match opcode {
                            0x1 => {
                                let _ = sent.send(String::from_utf8_lossy(&payload).into_owned());
                            }
                            0x8 => return,
                            _ => {}
                        }
                    }
                });
            }
        });
        (port, heard)
    }

    async fn next_heard(
        heard: &mut tokio::sync::mpsc::UnboundedReceiver<String>,
    ) -> Option<String> {
        loop {
            let text = tokio::time::timeout(std::time::Duration::from_secs(5), heard.recv())
                .await
                .ok()??;
            if !text.contains("session.update") {
                return Some(text);
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_session_door_carries_what_the_plane_reads_and_no_frame_it_cannot() {
        let _one = PUBLISHING.lock().await;
        let far_end = far_socket().await;
        let mut s = session_with(
            "serve-door-session-decode",
            Opts {
                far_end: Some(far_end),
                ..Opts::default()
            },
        )
        .await
        .expect("the build links the session door");
        let (router, plain) = (s.router.clone(), s.plain.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port");
        let addr = listener.local_addr().expect("its address");
        tokio::spawn(serve_listener(listener, router, s.framer.clone()));

        let route = REALTIME;
        let mut caller = tokio::net::TcpStream::connect(addr).await.expect("a dial");
        let nonce = ws_nonce();
        let open = format!(
            "GET {route} HTTP/1.1\r\nhost: {addr}\r\nupgrade: websocket\r\nconnection: Upgrade\r\n\
             sec-websocket-key: {nonce}\r\nsec-websocket-version: 13\r\n\
             authorization: Bearer {plain}\r\n\r\n"
        );
        caller.write_all(open.as_bytes()).await.expect("the open");
        let head = tokio::time::timeout(std::time::Duration::from_secs(10), read_head(&mut caller))
            .await
            .expect("the door answers the open")
            .expect("a head");
        assert!(
            head.starts_with("HTTP/1.1 101"),
            "the session opens: {head}"
        );

        let read = r#"{"type":"input_audio_buffer.append","audio":"AAAA"}"#;
        caller
            .write_all(&client_text(read))
            .await
            .expect("a readable event");
        let carried = next_heard(&mut s.heard)
            .await
            .expect("a client event the plane reads is carried to the provider");
        assert!(
            carried.contains("input_audio_buffer.append"),
            "the readable event, carried: {carried}"
        );

        caller
            .write_all(&client_text("not a frame {"))
            .await
            .expect("an unreadable frame");
        let after = r#"{"type":"input_audio_buffer.commit"}"#;
        caller
            .write_all(&client_text(after))
            .await
            .expect("a second readable event");
        let next = next_heard(&mut s.heard)
            .await
            .expect("the session stays open and carries the next readable event");
        assert!(
            !next.contains("not a frame"),
            "a frame the plane cannot read reaches the provider on no wire: {next}"
        );
        assert!(
            next.contains("input_audio_buffer.commit"),
            "the next event carried is the readable one after it: {next}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_accepted_session_drains_and_settles_while_its_external_caller_stays_connected() {
        use std::future::Future as _;
        let _one = PUBLISHING.lock().await;
        let detached = busbar_kernel::detached::DetachedTasks::new();
        busbar_kernel::detached::set_worker_detached(detached.clone());
        let (shutdown, stopped) = tokio::sync::watch::channel(false);
        busbar_kernel::detached::set_worker_shutdown(stopped);
        let far_end = far_socket_silent().await;
        let mut s = session_with(
            "serve-door-session-shutdown",
            Opts {
                far_end: Some(far_end),
                ..Opts::default()
            },
        )
        .await
        .expect("the build links the session door");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = s.router.clone();
        let serving = tokio::spawn(serve_listener(listener, router, s.framer.clone()));
        let mut caller = tokio::net::TcpStream::connect(address).await.unwrap();
        let nonce = ws_nonce();
        let open = format!(
            "GET {REALTIME} HTTP/1.1\r\nhost: {address}\r\nupgrade: websocket\r\nconnection: Upgrade\r\n\
             sec-websocket-key: {nonce}\r\nsec-websocket-version: 13\r\n\
             authorization: Bearer {}\r\n\r\n",
            s.plain,
        );
        caller.write_all(open.as_bytes()).await.unwrap();
        let head = tokio::time::timeout(
            busbar_kernel::detached::DETACHED_DRAIN_GRACE,
            read_head(&mut caller),
        )
        .await
        .expect("the real session opens")
        .unwrap();
        assert!(
            head.starts_with("HTTP/1.1 101"),
            "the session is accepted: {head}"
        );
        caller
            .write_all(&client_text(
                r#"{"type":"input_audio_buffer.append","audio":"AAAA"}"#,
            ))
            .await
            .unwrap();
        assert!(
            next_heard(&mut s.heard)
                .await
                .unwrap()
                .contains("input_audio_buffer.append"),
            "the accepted session has reached its actual provider before shutdown"
        );
        let mut draining = Box::pin(detached.drained());
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(
            draining.as_mut().poll(&mut context).is_pending(),
            "accepted session work belongs to the listener even while its caller is idle"
        );
        shutdown.send(true).unwrap();
        tokio::time::timeout(busbar_kernel::detached::DETACHED_DRAIN_GRACE, draining)
            .await
            .expect("the session closes through its normal driver path inside the existing grace");
        assert_eq!(
            s.lines(),
            1,
            "the accepted session settles exactly once before drain returns"
        );
        drop(caller);
        serving.abort();
        let _ = serving.await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_session_door_carries_turns_to_a_provider_that_does_not_speak_first() {
        let _one = PUBLISHING.lock().await;
        let far_end = far_socket_silent().await;
        let mut s = session_with(
            "serve-door-session-silent",
            Opts {
                far_end: Some(far_end),
                ..Opts::default()
            },
        )
        .await
        .expect("the build links the session door");
        let (router, plain) = (s.router.clone(), s.plain.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port");
        let addr = listener.local_addr().expect("its address");
        tokio::spawn(serve_listener(listener, router, s.framer.clone()));

        let route = REALTIME;
        let mut caller = tokio::net::TcpStream::connect(addr).await.expect("a dial");
        let nonce = ws_nonce();
        let open = format!(
            "GET {route} HTTP/1.1\r\nhost: {addr}\r\nupgrade: websocket\r\nconnection: Upgrade\r\n\
             sec-websocket-key: {nonce}\r\nsec-websocket-version: 13\r\n\
             authorization: Bearer {plain}\r\n\r\n"
        );
        caller.write_all(open.as_bytes()).await.expect("the open");
        let head = tokio::time::timeout(std::time::Duration::from_secs(10), read_head(&mut caller))
            .await
            .expect("the door answers the open")
            .expect("a head");
        assert!(
            head.starts_with("HTTP/1.1 101"),
            "the session opens: {head}"
        );

        for (event, kind) in [
            (
                r#"{"type":"input_audio_buffer.append","audio":"AAAA"}"#,
                "input_audio_buffer.append",
            ),
            (
                r#"{"type":"input_audio_buffer.commit"}"#,
                "input_audio_buffer.commit",
            ),
        ] {
            caller
                .write_all(&client_text(event))
                .await
                .expect("a readable event");
            let carried = next_heard(&mut s.heard).await.unwrap_or_else(|| {
                panic!("{kind} reaches a provider that answered the upgrade and never spoke")
            });
            assert!(
                carried.contains(kind),
                "the turns arrive in order: {kind}, carried: {carried}"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_session_door_bills_a_ga_mulaw_uplink_at_mulaw() {
        use base64::Engine as _;
        let _one = PUBLISHING.lock().await;
        let far_end = far_socket().await;
        let mut s = session_with(
            "serve-door-session-ga-format",
            Opts {
                far_end: Some(far_end),
                ..Opts::default()
            },
        )
        .await
        .expect("the build links the session door");
        let (router, plain) = (s.router.clone(), s.plain.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port");
        let addr = listener.local_addr().expect("its address");
        tokio::spawn(serve_listener(listener, router, s.framer.clone()));

        let route = REALTIME;
        let mut caller = tokio::net::TcpStream::connect(addr).await.expect("a dial");
        let nonce = ws_nonce();
        let open = format!(
            "GET {route} HTTP/1.1\r\nhost: {addr}\r\nupgrade: websocket\r\nconnection: Upgrade\r\n\
             sec-websocket-key: {nonce}\r\nsec-websocket-version: 13\r\n\
             authorization: Bearer {plain}\r\n\r\n"
        );
        caller.write_all(open.as_bytes()).await.expect("the open");
        let head = tokio::time::timeout(std::time::Duration::from_secs(10), read_head(&mut caller))
            .await
            .expect("the door answers the open")
            .expect("a head");
        assert!(
            head.starts_with("HTTP/1.1 101"),
            "the session opens: {head}"
        );

        let update = r#"{"type":"session.update","session":{"type":"realtime","audio":{"input":{"format":{"type":"audio/pcmu"}}}}}"#;
        caller
            .write_all(&client_text(update))
            .await
            .expect("the caller's GA update");

        let second = base64::engine::general_purpose::STANDARD.encode(vec![0x7f_u8; 8_000]);
        let append = format!(r#"{{"type":"input_audio_buffer.append","audio":"{second}"}}"#);
        for n in 0..6 {
            caller
                .write_all(&client_text(&append))
                .await
                .expect("a second of audio");
            let carried = next_heard(&mut s.heard)
                .await
                .unwrap_or_else(|| panic!("second {n} reaches the provider"));
            assert!(carried.contains("input_audio_buffer.append"), "{carried}");
        }

        caller
            .write_all(&[0x88, 0x80, 0x37, 0xFA, 0x21, 0x3D])
            .await
            .expect("the caller's close");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let (billed, sealed) = loop {
            let records = s.audit_records();
            let seconds = records.iter().find_map(|r| {
                r.usage
                    .lines
                    .iter()
                    .find(|l| l.class.as_str() == AUDIO_CLASS)
                    .map(|l| l.quantity)
            });
            if seconds.is_some() || std::time::Instant::now() > deadline {
                let sealed: Vec<String> = records
                    .iter()
                    .map(|r| format!("{:?} {:?}", r.outcome.unit_end, r.usage.lines))
                    .collect();
                break (seconds, sealed);
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        };
        assert_eq!(
            billed,
            Some(6.into()),
            "six seconds of µ-law the provider read are six billed seconds; sealed: {sealed:?}"
        );
    }

    async fn post_offer(s: &SessionDoor, offer: &[u8]) -> (StatusCode, String, Vec<u8>) {
        use tower::ServiceExt as _;
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/realtime/calls")
            .header("content-type", "application/sdp")
            .header("authorization", format!("Bearer {}", s.plain))
            .body(axum::body::Body::from(offer.to_vec()))
            .expect("a request");
        let response = tokio::time::timeout(ANSWER_BOUND, s.router.clone().oneshot(req))
            .await
            .expect("the SDP door answers within the bound")
            .expect("the router answers");
        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        (status, content_type, body_of(response).await.to_vec())
    }

    async fn session_open_refusals(s: &SessionDoor) -> usize {
        busbar_kernel::audit::auditlog::flush_pending()
            .await
            .expect("the native audit stream confirms the observed refusal rows");
        busbar_kernel::audit::auditlog::AUDIT_LOG
            .list_filtered(0, 1000, Some("door.session_open"), None)
            .into_iter()
            .filter(|e| e.principal == s.key.id && e.outcome == "rejected")
            .count()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_offer_the_door_cannot_open_media_for_is_refused_explained_and_audited() {
        let _one = PUBLISHING.lock().await;
        let explained = |body: &[u8]| -> String {
            let v: serde_json::Value =
                serde_json::from_slice(body).unwrap_or_else(|_| panic!("a JSON refusal: {body:?}"));
            v["error"]["message"]
                .as_str()
                .unwrap_or_else(|| panic!("an error message: {v}"))
                .to_owned()
        };
        let mut s = session_with(
            "serve-door-session-no-media",
            Opts {
                no_media: true,
                ..Opts::default()
            },
        )
        .await
        .expect("the build links the session door");
        busbar_kernel::boot::hydrate_all(&s.app)
            .await
            .expect("the actual boot registers the durable audit stream before serving units");
        let before = session_open_refusals(&s).await;
        let (_browser, offer) = RtcPeer::offer(
            s.connector
                .framer_for("webrtc")
                .expect("the real WebRTC framer"),
        )
        .await;
        let (status, content_type, body) = post_offer(&s, &offer).await;
        assert_eq!(status.as_u16(), 404, "{}", String::from_utf8_lossy(&body));
        assert!(content_type.contains("json"), "{content_type}");
        let message = explained(&body);
        assert!(
            message.contains(".media.listen"),
            "names the missing setting: {message}"
        );
        assert_eq!(session_open_refusals(&s).await, before + 1, "audited");
        assert!(s.heard.try_recv().is_err(), "nothing dialled");

        let mut s = session_with("serve-door-session-no-lines", Opts::default())
            .await
            .expect("the build links the session door");
        busbar_kernel::boot::hydrate_all(&s.app)
            .await
            .expect("the actual boot registers the durable audit stream before serving units");
        let before = session_open_refusals(&s).await;
        let (status, content_type, body) = post_offer(
            &s,
            b"v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=no-lines\r\nt=0 0\r\n",
        )
        .await;
        assert_eq!(status.as_u16(), 400, "{}", String::from_utf8_lossy(&body));
        assert!(content_type.contains("json"), "{content_type}");
        let message = explained(&body);
        assert!(
            message.to_ascii_lowercase().contains("m-line")
                || message.to_ascii_lowercase().contains("media"),
            "names why the offer opens no media: {message}"
        );
        assert_eq!(session_open_refusals(&s).await, before + 1, "audited");
        assert!(s.heard.try_recv().is_err(), "nothing dialled");
    }

    async fn next_report(
        heard: &mut tokio::sync::mpsc::UnboundedReceiver<String>,
    ) -> Option<String> {
        loop {
            let text = tokio::time::timeout(std::time::Duration::from_secs(5), heard.recv())
                .await
                .ok()??;
            if !text.contains("session.update") {
                return Some(text);
            }
        }
    }

    async fn open_socket(
        addr: std::net::SocketAddr,
        route: &str,
        plain: &str,
    ) -> (tokio::net::TcpStream, Option<String>) {
        let mut caller = tokio::net::TcpStream::connect(addr).await.expect("a dial");
        let nonce = ws_nonce();
        let open = format!(
            "GET {route} HTTP/1.1\r\nhost: {addr}\r\nupgrade: websocket\r\nconnection: Upgrade\r\n\
             sec-websocket-key: {nonce}\r\nsec-websocket-version: 13\r\n\
             authorization: Bearer {plain}\r\n\r\n"
        );
        caller.write_all(open.as_bytes()).await.expect("the open");
        let head = tokio::time::timeout(std::time::Duration::from_secs(10), read_head(&mut caller))
            .await
            .expect("the door answers the open");
        (caller, head)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_session_door_sideband_attaches_only_to_a_placed_call() {
        let _one = PUBLISHING.lock().await;
        let mut s = session_with("serve-door-session-sideband", Opts { ..Opts::default() })
            .await
            .expect("the build links the session door");
        let (router, plain) = (s.router.clone(), s.plain.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port");
        let addr = listener.local_addr().expect("its address");
        let served = router.clone();
        tokio::spawn(serve_listener(listener, served, s.framer.clone()));
        let append = r#"{"type":"input_audio_buffer.append","audio":"AAAA"}"#;

        let unplaced = SIDEBAND.replace("{call_id}", "rtc_unplaced0001");
        let (mut caller, head) = open_socket(addr, &unplaced, &plain).await;
        if head
            .as_deref()
            .is_some_and(|h| h.starts_with("HTTP/1.1 101"))
        {
            let _ = caller.write_all(&client_text(append)).await;
        }
        let stray = next_report(&mut s.heard).await;
        assert!(
            stray.is_none(),
            "a sideband naming a call no offer placed reaches the provider on no wire: \
             open {head:?}, provider heard {stray:?}"
        );
        drop(caller);

        let (status, response) = s.signal(&plain).await;
        assert_eq!(status.as_u16(), 201, "the actual peer offer placed a call");
        let location = response
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("location")
                    .then(|| value.trim().to_owned())
            })
            .expect("the caller is told where its owned call lives");
        let call = location
            .strip_prefix("/v1/realtime/calls/")
            .expect("the call location stays on this door");
        assert!(
            call.starts_with("rtc_")
                && call
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        );

        s.wait_media_ready().await;
        let posted = next_report(&mut s.heard)
            .await
            .expect("the offer reached the provider");
        assert!(posted.starts_with("POST /v1/realtime/calls "), "{posted}");

        let attached = SIDEBAND.replace("{call_id}", call);
        let (mut caller, head) = open_socket(addr, &attached, &plain).await;
        assert!(
            head.as_deref()
                .is_some_and(|h| h.starts_with("HTTP/1.1 101")),
            "the sideband to the caller's own call opens: {head:?}"
        );
        caller
            .write_all(&client_text(append))
            .await
            .expect("a readable event");
        let dial = next_report(&mut s.heard)
            .await
            .expect("the sideband dials the provider");
        assert!(
            dial.starts_with(&format!(
                "DIAL GET /v1/realtime?call_id={PROVIDER_CALL_ID} "
            )),
            "the sideband attaches to the call it names: {dial}"
        );
        let carried = next_report(&mut s.heard)
            .await
            .expect("the caller's event reaches the call");
        assert!(
            carried.starts_with("TEXT ") && carried.contains("input_audio_buffer.append"),
            "{carried}"
        );
        caller
            .shutdown()
            .await
            .expect("the actual sideband caller closes");
        drop(caller);
        s.finish_media().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_session_door_dials_the_session_models_upstream_name() {
        let _one = PUBLISHING.lock().await;
        let mut s = session_with(
            "serve-door-session-upstream",
            Opts {
                upstream_model: Some("gpt-realtime-wire"),
                ..Opts::default()
            },
        )
        .await
        .expect("the build links the session door");
        let (router, plain) = (s.router.clone(), s.plain.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port");
        let addr = listener.local_addr().expect("its address");
        tokio::spawn(serve_listener(listener, router, s.framer.clone()));
        let (mut caller, head) = open_socket(addr, REALTIME, &plain).await;
        assert!(
            head.as_deref()
                .is_some_and(|h| h.starts_with("HTTP/1.1 101")),
            "the plain session opens: {head:?}"
        );
        caller
            .write_all(&client_text(
                r#"{"type":"input_audio_buffer.append","audio":"AAAA"}"#,
            ))
            .await
            .expect("a readable event");
        let dial = next_report(&mut s.heard)
            .await
            .expect("the session dials the provider");
        assert!(
            dial.starts_with("DIAL GET /v1/realtime?model=gpt-realtime-wire "),
            "the dial names the upstream model, never the catalog alias: {dial}"
        );
    }
}
