use std::{fmt::Debug, future::Future};

use futures_util::StreamExt;
use kube::{runtime::watcher::Event, Api, Resource};
use serde::de::DeserializeOwned;
use tokio_util::sync::CancellationToken;
use tracing::{error, warn};

pub trait ApiWatcherCallbacks<T>: Send + 'static {
    fn apply(&self, obj: Vec<T>) -> impl Future<Output = anyhow::Result<()>> + Send;
    fn delete(&self, obj: Vec<T>) -> impl Future<Output = anyhow::Result<()>> + Send;
}

pub async fn api_watcher<K, C>(api: Api<K>, callbacks: C, cancel: CancellationToken)
where
    K: Clone + Debug + DeserializeOwned + Send + Sync + 'static + Resource,
    C: ApiWatcherCallbacks<K>,
{
    let mut stream =
        kube::runtime::watcher::watcher(api, kube::runtime::watcher::Config::default()).boxed();

    let mut initial = vec![];

    loop {
        tokio::select! {
            msg = stream.next() => {
                let Some(msg) = msg else {
                    break;
                };
                match msg {
                    Ok(Event::Apply(obj)) => {
                        if let Err(err) = callbacks.apply(vec![obj]).await {
                            warn!(?err, "error applying watched k8s resource");
                        }
                    }
                    Ok(Event::Delete(obj)) => {
                        if let Err(err) = callbacks.delete(vec![obj]).await {
                            warn!(?err, "error deleting watched k8s resource");
                        }
                    }
                    Ok(Event::Init) => {
                        initial = vec![];
                    }
                    Ok(Event::InitApply(obj)) => {
                        initial.push(obj);
                    }
                    Ok(Event::InitDone) => {
                        if let Err(err) = callbacks.apply(initial).await {
                            warn!(?err, "error applying watched k8s resource");
                        }
                        initial = vec![];
                    }
                    Err(err) => {
                        error!(?err, "k8s watcher error");
                    }
                }
            }
            _ = cancel.cancelled() =>  {
                break
            }
        }
    }
}
