use std::sync::Mutex;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::{ConnectorError, ConnectorStatus, InboundEvent};

#[derive(Default)]
pub(crate) struct ConnectorRuntimeState {
    inner: Mutex<RuntimeSlot>,
}

#[derive(Default)]
struct RuntimeSlot {
    status: Option<ConnectorStatus>,
    inbound: Option<mpsc::Sender<InboundEvent>>,
    cancel: Option<CancellationToken>,
    task: Option<JoinHandle<()>>,
}

impl ConnectorRuntimeState {
    pub(crate) fn status(&self) -> ConnectorStatus {
        self.lock().status.clone().unwrap_or(ConnectorStatus::Stopped)
    }

    pub(crate) fn begin(&self, inbound: Option<mpsc::Sender<InboundEvent>>) -> Result<CancellationToken, ConnectorError> {
        let mut slot = self.lock();
        if matches!(slot.status, Some(ConnectorStatus::Running | ConnectorStatus::Starting)) {
            return Err(ConnectorError::Config("connector is already running".to_owned()));
        }
        let cancel = CancellationToken::new();
        slot.cancel = Some(cancel.clone());
        slot.inbound = inbound;
        slot.status = Some(ConnectorStatus::Running);
        Ok(cancel)
    }

    pub(crate) fn attach_task(&self, task: JoinHandle<()>) {
        self.lock().task = Some(task);
    }

    pub(crate) fn is_running(&self) -> bool {
        matches!(self.lock().status, Some(ConnectorStatus::Running))
    }

    pub(crate) async fn end(&self) {
        let (cancel, task) = {
            let mut slot = self.lock();
            slot.status = Some(ConnectorStatus::Stopped);
            slot.inbound = None;
            (slot.cancel.take(), slot.task.take())
        };
        if let Some(cancel) = cancel {
            cancel.cancel();
        }
        if let Some(task) = task {
            let _finished = task.await;
        }
    }

    pub(crate) async fn emit(&self, event: InboundEvent) -> Result<(), ConnectorError> {
        let sender = self.lock().inbound.clone().ok_or(ConnectorError::NotRunning)?;
        sender.send(event).await.map_err(|_| ConnectorError::NotRunning)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, RuntimeSlot> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
