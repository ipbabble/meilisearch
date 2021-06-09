use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::fs;
use tokio::sync::RwLock;
use tokio::task::spawn_blocking;
use uuid::Uuid;

use super::{IndexError, IndexResult};
use crate::index::Index;

type AsyncMap<K, V> = Arc<RwLock<HashMap<K, V>>>;

#[async_trait::async_trait]
pub trait IndexStore {
    async fn create(&self, uuid: Uuid, primary_key: Option<String>) -> IndexResult<Index>;
    async fn get(&self, uuid: Uuid) -> IndexResult<Option<Index>>;
    async fn delete(&self, uuid: Uuid) -> IndexResult<Option<Index>>;
}

pub struct MapIndexStore {
    index_store: AsyncMap<Uuid, Index>,
    path: PathBuf,
    index_size: usize,
}

impl MapIndexStore {
    pub fn new(path: impl AsRef<Path>, index_size: usize) -> Self {
        let path = path.as_ref().join("indexes/");
        let index_store = Arc::new(RwLock::new(HashMap::new()));
        Self {
            index_store,
            path,
            index_size,
        }
    }
}

#[async_trait::async_trait]
impl IndexStore for MapIndexStore {
    async fn create(&self, uuid: Uuid, primary_key: Option<String>) -> IndexResult<Index> {
        let mut lock = self.index_store.write().await;

        if let Some(index) = lock.get(&uuid) {
            return Ok(index.clone())
        }
        let path = self.path.join(format!("index-{}", uuid));
        if path.exists() {
            return Err(IndexError::IndexAlreadyExists);
        }

        let index_size = self.index_size;
        let index = spawn_blocking(move || -> IndexResult<Index> {
            let index = Index::open(path, index_size)?;
            if let Some(primary_key) = primary_key {
                let mut txn = index.write_txn()?;
                index.put_primary_key(&mut txn, &primary_key)?;
                txn.commit()?;
            }
            Ok(index)
        })
        .await??;

        lock.insert(uuid, index.clone());

        Ok(index)
    }

    async fn get(&self, uuid: Uuid) -> IndexResult<Option<Index>> {
        let guard = self.index_store.read().await;
        match guard.get(&uuid) {
            Some(index) => Ok(Some(index.clone())),
            None => {
                // drop the guard here so we can perform the write after without deadlocking;
                drop(guard);
                let path = self.path.join(format!("index-{}", uuid));
                if !path.exists() {
                    return Ok(None);
                }

                let index_size = self.index_size;
                let index = spawn_blocking(move || Index::open(path, index_size)).await??;
                self.index_store.write().await.insert(uuid, index.clone());
                Ok(Some(index))
            }
        }
    }

    async fn delete(&self, uuid: Uuid) -> IndexResult<Option<Index>> {
        let db_path = self.path.join(format!("index-{}", uuid));
        fs::remove_dir_all(db_path).await?;
        let index = self.index_store.write().await.remove(&uuid);
        Ok(index)
    }
}
