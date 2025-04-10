// Copyright (c) 2023 - 2025 Restate Software, Inc., Restate GmbH.
// All rights reserved.
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of the Change Date specified in that file, in accordance with
// the Business Source License, use of this software will be governed
// by the Apache License, Version 2.0.

//! Network processing for log-server
//!
//! We maintain a stream per message type for fine-grain per-message-type control over the queue
//! depth, head-of-line blocking issues, and priority of consumption.
use std::collections::{HashMap, hash_map};

use anyhow::Context;
use tokio::task::JoinSet;
use tokio_stream::StreamExt as TokioStreamExt;
use tracing::{debug, trace};

use restate_core::cancellation_watcher;
use restate_core::network::{Incoming, MessageRouterBuilder, MessageStream};
use restate_types::config::Configuration;
use restate_types::health::HealthStatus;
use restate_types::live::Live;
use restate_types::logs::LogletId;
use restate_types::net::log_server::*;
use restate_types::nodes_config::StorageState;
use restate_types::protobuf::common::LogServerStatus;

use crate::loglet_worker::{LogletWorker, LogletWorkerHandle};
use crate::logstore::LogStore;
use crate::metadata::LogletStateMap;

const DEFAULT_WRITERS_CAPACITY: usize = 128;

type LogletWorkerMap = HashMap<LogletId, LogletWorkerHandle>;

pub struct RequestPump {
    _configuration: Live<Configuration>,
    store_stream: MessageStream<Store>,
    release_stream: MessageStream<Release>,
    seal_stream: MessageStream<Seal>,
    get_loglet_info_stream: MessageStream<GetLogletInfo>,
    get_records_stream: MessageStream<GetRecords>,
    trim_stream: MessageStream<Trim>,
    wait_for_tail_stream: MessageStream<WaitForTail>,
    get_digest_stream: MessageStream<GetDigest>,
}

impl RequestPump {
    pub fn new(
        mut configuration: Live<Configuration>,
        router_builder: &mut MessageRouterBuilder,
    ) -> Self {
        let queue_length = configuration
            .live_load()
            .log_server
            .incoming_network_queue_length
            .into();
        // We divide requests into two priority categories.
        let store_stream = router_builder.subscribe_to_stream(queue_length);
        let release_stream = router_builder.subscribe_to_stream(queue_length);
        let seal_stream = router_builder.subscribe_to_stream(queue_length);
        let get_loglet_info_stream = router_builder.subscribe_to_stream(queue_length);
        let get_records_stream = router_builder.subscribe_to_stream(queue_length);
        let trim_stream = router_builder.subscribe_to_stream(queue_length);
        let wait_for_tail_stream = router_builder.subscribe_to_stream(queue_length);
        let get_digest_stream = router_builder.subscribe_to_stream(queue_length);
        Self {
            _configuration: configuration,
            store_stream,
            release_stream,
            seal_stream,
            get_loglet_info_stream,
            get_records_stream,
            trim_stream,
            wait_for_tail_stream,
            get_digest_stream,
        }
    }

    /// Starts the main processing loop, exits on error or shutdown.
    pub async fn run<S>(
        self,
        health_status: HealthStatus<LogServerStatus>,
        log_store: S,
        state_map: LogletStateMap,
        _storage_state: StorageState,
    ) -> anyhow::Result<()>
    where
        S: LogStore + Clone + Sync + Send + 'static,
    {
        let RequestPump {
            mut store_stream,
            mut release_stream,
            mut seal_stream,
            mut get_loglet_info_stream,
            mut get_records_stream,
            mut trim_stream,
            mut wait_for_tail_stream,
            mut get_digest_stream,
            ..
        } = self;

        let mut shutdown = std::pin::pin!(cancellation_watcher());

        let mut loglet_workers = HashMap::with_capacity(DEFAULT_WRITERS_CAPACITY);

        health_status.update(LogServerStatus::Ready);

        // We need to dispatch this work to the right loglet worker as quickly as possible
        // to avoid blocking the message handler.
        //
        // We only block on loading loglet state from logstore, if this proves to be a bottle-neck (it
        // shouldn't) then we can offload this operation to a background task.
        loop {
            // Ordered by priority of message types
            tokio::select! {
                biased;
                _ = &mut shutdown => {
                    health_status.update(LogServerStatus::Stopping);
                    // stop accepting messages
                    drop(wait_for_tail_stream);
                    drop(store_stream);
                    drop(release_stream);
                    drop(seal_stream);
                    drop(get_loglet_info_stream);
                    drop(get_records_stream);
                    drop(trim_stream);
                    drop(get_digest_stream);
                    // shutdown all workers.
                    Self::shutdown(loglet_workers).await;
                    health_status.update(LogServerStatus::Unknown);
                    return Ok(());
                }
                Some(get_digest) = get_digest_stream.next() => {
                    // find the worker or create one.
                    // enqueue.
                    let worker = Self::find_or_create_worker(
                        get_digest.body().header.loglet_id,
                        &log_store,
                        &state_map,
                        &mut loglet_workers,
                    ).await?;
                    Self::on_get_digest(worker, get_digest);
                }
                Some(wait_for_tail) = wait_for_tail_stream.next() => {
                    // find the worker or create one.
                    // enqueue.
                    let worker = Self::find_or_create_worker(
                        wait_for_tail.body().header.loglet_id,
                        &log_store,
                        &state_map,
                        &mut loglet_workers,
                    ).await?;
                    Self::on_wait_for_tail(worker, wait_for_tail);
                }
                Some(release) = release_stream.next() => {
                    // find the worker or create one.
                    // enqueue.
                    let worker = Self::find_or_create_worker(
                        release.body().header.loglet_id,
                        &log_store,
                        &state_map,
                        &mut loglet_workers,
                    ).await?;
                    Self::on_release(worker, release);
                }
                Some(seal) = seal_stream.next() => {
                    // find the worker or create one.
                    // enqueue.
                    let worker = Self::find_or_create_worker(
                        seal.body().header.loglet_id,
                        &log_store,
                        &state_map,
                        &mut loglet_workers,
                    ).await?;
                    Self::on_seal(worker, seal);
                }
                Some(get_loglet_info) = get_loglet_info_stream.next() => {
                    // find the worker or create one.
                    // enqueue.
                    let worker = Self::find_or_create_worker(
                        get_loglet_info.body().header.loglet_id,
                        &log_store,
                        &state_map,
                        &mut loglet_workers,
                    ).await?;
                    Self::on_get_loglet_info(worker, get_loglet_info);
                }
                Some(get_records) = get_records_stream.next() => {
                    // find the worker or create one.
                    // enqueue.
                    let worker = Self::find_or_create_worker(
                        get_records.body().header.loglet_id,
                        &log_store,
                        &state_map,
                        &mut loglet_workers,
                    ).await?;
                    Self::on_get_records(worker, get_records);
                }
                Some(trim) = trim_stream.next() => {
                    // find the worker or create one.
                    // enqueue.
                    let worker = Self::find_or_create_worker(
                        trim.body().header.loglet_id,
                        &log_store,
                        &state_map,
                        &mut loglet_workers,
                    ).await?;
                    Self::on_trim(worker, trim);
                }
                Some(store) = store_stream.next() => {
                    // find the worker or create one.
                    // enqueue.
                    let worker = Self::find_or_create_worker(
                        store.body().header.loglet_id,
                        &log_store,
                        &state_map,
                        &mut loglet_workers,
                    ).await?;
                    Self::on_store(worker, store);
                }
            }
        }
    }

    pub async fn shutdown(loglet_workers: LogletWorkerMap) {
        // stop all writers
        let mut tasks = JoinSet::new();
        for (_, task) in loglet_workers {
            tasks.spawn(task.cancel());
        }
        // await all tasks to shutdown
        let _ = tasks.join_all().await;
        trace!("All loglet workers have terminated");
    }

    fn on_get_digest(worker: &LogletWorkerHandle, msg: Incoming<GetDigest>) {
        if let Err(msg) = worker.enqueue_get_digest(msg) {
            let peer = msg.peer();
            // worker has crashed or shutdown in progress. Notify the sender and drop the message.
            if !msg.to_rpc_response(Digest::empty()).try_send() {
                debug!(%peer, "Failed to respond to GetDigest message with status Disabled");
            }
        }
    }

    fn on_wait_for_tail(worker: &LogletWorkerHandle, msg: Incoming<WaitForTail>) {
        if let Err(msg) = worker.enqueue_wait_for_tail(msg) {
            let peer = msg.peer();
            // worker has crashed or shutdown in progress. Notify the sender and drop the message.
            if !msg.to_rpc_response(TailUpdated::empty()).try_send() {
                debug!(%peer, "Failed to respond to WaitForTail message with status Disabled");
            }
        }
    }

    fn on_store(worker: &LogletWorkerHandle, msg: Incoming<Store>) {
        if let Err(msg) = worker.enqueue_store(msg) {
            let peer = msg.peer();
            // worker has crashed or shutdown in progress. Notify the sender and drop the message.
            if !msg.to_rpc_response(Stored::empty()).try_send() {
                debug!(%peer, "Failed to respond to Store message with status Disabled");
            }
        }
    }

    fn on_release(worker: &LogletWorkerHandle, msg: Incoming<Release>) {
        if let Err(msg) = worker.enqueue_release(msg) {
            let peer = msg.peer();
            // worker has crashed or shutdown in progress. Notify the sender and drop the message.
            if !msg.to_rpc_response(Released::empty()).try_send() {
                debug!(%peer, "Failed to respond to Release message with status Disabled");
            }
        }
    }

    fn on_seal(worker: &LogletWorkerHandle, msg: Incoming<Seal>) {
        if let Err(msg) = worker.enqueue_seal(msg) {
            let peer = msg.peer();
            // worker has crashed or shutdown in progress. Notify the sender and drop the message.
            if !msg.to_rpc_response(Sealed::empty()).try_send() {
                debug!(%peer, "Failed to respond to Seal message with status Disabled");
            }
        }
    }

    fn on_get_loglet_info(worker: &LogletWorkerHandle, msg: Incoming<GetLogletInfo>) {
        if let Err(msg) = worker.enqueue_get_loglet_info(msg) {
            let peer = msg.peer();
            // worker has crashed or shutdown in progress. Notify the sender and drop the message.
            if !msg.to_rpc_response(LogletInfo::empty()).try_send() {
                debug!(%peer, "Failed to respond to GetLogletInfo message with status Disabled");
            }
        }
    }

    fn on_get_records(worker: &LogletWorkerHandle, msg: Incoming<GetRecords>) {
        if let Err(msg) = worker.enqueue_get_records(msg) {
            let next_offset = msg.body().from_offset;
            let peer = msg.peer();
            // worker has crashed or shutdown in progress. Notify the sender and drop the message.
            if !msg.to_rpc_response(Records::empty(next_offset)).try_send() {
                debug!(%peer, "Failed to respond to GetRecords message with status Disabled");
            }
        }
    }

    fn on_trim(worker: &LogletWorkerHandle, msg: Incoming<Trim>) {
        if let Err(msg) = worker.enqueue_trim(msg) {
            let peer = msg.peer();
            // worker has crashed or shutdown in progress. Notify the sender and drop the message.
            if !msg.to_rpc_response(Trimmed::empty()).try_send() {
                debug!(%peer, "Failed to respond to Trim message with status Disabled");
            }
        }
    }

    async fn find_or_create_worker<'a, S: LogStore>(
        loglet_id: LogletId,
        log_store: &S,
        state_map: &LogletStateMap,
        loglet_workers: &'a mut LogletWorkerMap,
    ) -> anyhow::Result<&'a LogletWorkerHandle> {
        if let hash_map::Entry::Vacant(e) = loglet_workers.entry(loglet_id) {
            let state = state_map
                .get_or_load(loglet_id, log_store)
                .await
                .context("cannot load loglet state map from logstore")?;
            let handle = LogletWorker::start(loglet_id, log_store.clone(), state.clone())?;
            e.insert(handle);
        }

        Ok(loglet_workers.get(&loglet_id).unwrap())
    }
}
