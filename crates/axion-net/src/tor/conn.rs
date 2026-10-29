//! An Arti stream as a blocking [`Conn`].
//!
//! The loopback servers of this crate are thread based; Arti is async. A stream is split in
//! two halves, and each blocking call drives its half on the private runtime
//! (`Handle::block_on`). Every call also watches the cancel signals: "New circuit" for the
//! site, and the tunnel stopping, close the connection instead of leaving it on an old circuit.

use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex, PoisonError};

use arti_client::{DataReader, DataStream, DataWriter};
use futures::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::runtime::Handle;
use tokio::sync::watch;

use crate::conn::Conn;

/// Completes when one of the signals is `true`. A signal whose sender is gone never fires.
pub(super) async fn cancelled(signals: &mut [watch::Receiver<bool>]) {
    async fn one(rx: &mut watch::Receiver<bool>) {
        if rx.wait_for(|v| *v).await.is_err() {
            std::future::pending::<()>().await;
        }
    }
    if signals.is_empty() {
        return std::future::pending().await;
    }
    let waits: Vec<_> = signals.iter_mut().map(|rx| Box::pin(one(rx))).collect();
    futures::future::select_all(waits).await;
}

fn aborted() -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionAborted, "the Tor circuit was closed")
}

pub(super) struct TorConn {
    handle: Handle,
    reader: Arc<Mutex<DataReader>>,
    writer: Arc<Mutex<DataWriter>>,
    cancels: Vec<watch::Receiver<bool>>,
}

impl TorConn {
    pub(super) fn new(handle: Handle, stream: DataStream, cancels: Vec<watch::Receiver<bool>>) -> Self {
        let (reader, writer) = stream.split();
        Self { handle, reader: Arc::new(Mutex::new(reader)), writer: Arc::new(Mutex::new(writer)), cancels }
    }
}

impl Read for TorConn {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut reader = self.reader.lock().unwrap_or_else(PoisonError::into_inner);
        let cancels = &mut self.cancels;
        self.handle.block_on(async {
            tokio::select! {
                biased;
                () = cancelled(cancels) => Err(aborted()),
                n = reader.read(buf) => n,
            }
        })
    }
}

impl Write for TorConn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut writer = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        let cancels = &mut self.cancels;
        self.handle.block_on(async {
            tokio::select! {
                biased;
                () = cancelled(cancels) => Err(aborted()),
                r = async { writer.write_all(buf).await?; writer.flush().await } => r.map(|()| buf.len()),
            }
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut writer = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        self.handle.block_on(writer.flush())
    }
}

impl Conn for TorConn {
    fn try_clone_conn(&self) -> io::Result<Box<dyn Conn>> {
        Ok(Box::new(TorConn {
            handle: self.handle.clone(),
            reader: Arc::clone(&self.reader),
            writer: Arc::clone(&self.writer),
            cancels: self.cancels.clone(),
        }))
    }

    fn shutdown_write(&self) -> io::Result<()> {
        let mut writer = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        self.handle.block_on(writer.close())
    }
}
