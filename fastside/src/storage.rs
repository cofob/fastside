use std::{collections::HashMap, fmt::Debug};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::RwLock;

use crate::crawler::CrawledServices;

#[cfg(feature = "native")]
use fastside_shared::config::{StorageBackend, StorageConfig};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CrawlSnapshot {
    pub crawled_services: CrawledServices,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct InstanceReputation {
    pub upvotes: u64,
    pub downvotes: u64,
}

impl InstanceReputation {
    pub fn weight(self, minimum: f64, maximum: f64) -> f64 {
        let weight = (self.upvotes as f64 + 1.0) / (self.downvotes as f64 + 1.0);
        weight.clamp(minimum, maximum)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum VoteDirection {
    Up,
    Down,
}

#[derive(Debug, Error)]
#[error("storage error: {0}")]
pub struct StorageError(pub String);

#[async_trait]
pub trait StateStore: Debug + Send + Sync {
    async fn load_crawl_snapshot(&self) -> Result<Option<CrawlSnapshot>, StorageError>;

    async fn save_crawl_snapshot(&self, snapshot: &CrawlSnapshot) -> Result<(), StorageError>;

    async fn get_reputations(
        &self,
        instances: &[String],
    ) -> Result<HashMap<String, InstanceReputation>, StorageError>;

    async fn apply_vote(
        &self,
        instance: &str,
        direction: VoteDirection,
    ) -> Result<InstanceReputation, StorageError>;
}

#[derive(Debug, Default)]
pub struct MemoryStateStore {
    snapshot: RwLock<Option<CrawlSnapshot>>,
    reputations: RwLock<HashMap<String, InstanceReputation>>,
}

#[async_trait]
impl StateStore for MemoryStateStore {
    async fn load_crawl_snapshot(&self) -> Result<Option<CrawlSnapshot>, StorageError> {
        Ok(self.snapshot.read().await.clone())
    }

    async fn save_crawl_snapshot(&self, snapshot: &CrawlSnapshot) -> Result<(), StorageError> {
        *self.snapshot.write().await = Some(snapshot.clone());
        Ok(())
    }

    async fn get_reputations(
        &self,
        instances: &[String],
    ) -> Result<HashMap<String, InstanceReputation>, StorageError> {
        let reputations = self.reputations.read().await;
        Ok(instances
            .iter()
            .filter_map(|instance| {
                reputations
                    .get(instance)
                    .copied()
                    .map(|reputation| (instance.clone(), reputation))
            })
            .collect())
    }

    async fn apply_vote(
        &self,
        instance: &str,
        direction: VoteDirection,
    ) -> Result<InstanceReputation, StorageError> {
        let mut reputations = self.reputations.write().await;
        let reputation = reputations.entry(instance.to_owned()).or_default();
        match direction {
            VoteDirection::Up => reputation.upvotes = reputation.upvotes.saturating_add(1),
            VoteDirection::Down => {
                reputation.downvotes = reputation.downvotes.saturating_add(1);
            }
        }
        Ok(*reputation)
    }
}

#[cfg(feature = "native")]
pub async fn create_native_store(
    config: &StorageConfig,
) -> Result<std::sync::Arc<dyn StateStore>, StorageError> {
    match config.backend {
        StorageBackend::Auto | StorageBackend::Sqlite => Ok(std::sync::Arc::new(
            native::SqliteStateStore::connect(&config.sqlite).await?,
        )),
        StorageBackend::Redis => Ok(std::sync::Arc::new(
            native::RedisStateStore::connect(&config.redis).await?,
        )),
        StorageBackend::Cloudflare => Err(StorageError(
            "Cloudflare storage is not available in the native server".to_owned(),
        )),
    }
}

#[cfg(feature = "native")]
mod native {
    use std::{collections::HashMap, str::FromStr};

    use async_trait::async_trait;
    use fastside_shared::config::{RedisStorageConfig, SqliteStorageConfig};
    use redis::aio::ConnectionManager;
    use sqlx::{
        QueryBuilder, Row, Sqlite, SqlitePool,
        sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
    };

    use super::{CrawlSnapshot, InstanceReputation, StateStore, StorageError, VoteDirection};

    const CRAWL_SNAPSHOT_KEY: &str = "crawl-snapshot-v1";

    #[derive(Debug)]
    pub struct SqliteStateStore {
        pool: SqlitePool,
    }

    impl SqliteStateStore {
        pub async fn connect(config: &SqliteStorageConfig) -> Result<Self, StorageError> {
            let options = SqliteConnectOptions::from_str(&format!("sqlite:{}", config.path))
                .map_err(|error| StorageError(error.to_string()))?
                .create_if_missing(true)
                .journal_mode(SqliteJournalMode::Wal)
                .busy_timeout(config.busy_timeout);
            let pool = SqlitePoolOptions::new()
                .max_connections(config.max_connections.max(1))
                .connect_with(options)
                .await
                .map_err(|error| StorageError(error.to_string()))?;
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS app_state (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
            )
            .execute(&pool)
            .await
            .map_err(|error| StorageError(error.to_string()))?;
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS instance_reputation (instance TEXT PRIMARY KEY, upvotes INTEGER NOT NULL DEFAULT 0, downvotes INTEGER NOT NULL DEFAULT 0)",
            )
            .execute(&pool)
            .await
            .map_err(|error| StorageError(error.to_string()))?;
            Ok(Self { pool })
        }
    }

    #[async_trait]
    impl StateStore for SqliteStateStore {
        async fn load_crawl_snapshot(&self) -> Result<Option<CrawlSnapshot>, StorageError> {
            let value =
                sqlx::query_scalar::<_, String>("SELECT value FROM app_state WHERE key = ?")
                    .bind(CRAWL_SNAPSHOT_KEY)
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(|error| StorageError(error.to_string()))?;
            value
                .map(|value| {
                    serde_json::from_str(&value).map_err(|error| StorageError(error.to_string()))
                })
                .transpose()
        }

        async fn save_crawl_snapshot(&self, snapshot: &CrawlSnapshot) -> Result<(), StorageError> {
            let value =
                serde_json::to_string(snapshot).map_err(|error| StorageError(error.to_string()))?;
            sqlx::query(
                "INSERT INTO app_state (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            )
            .bind(CRAWL_SNAPSHOT_KEY)
            .bind(value)
            .execute(&self.pool)
            .await
            .map_err(|error| StorageError(error.to_string()))?;
            Ok(())
        }

        async fn get_reputations(
            &self,
            instances: &[String],
        ) -> Result<HashMap<String, InstanceReputation>, StorageError> {
            if instances.is_empty() {
                return Ok(HashMap::new());
            }
            let mut query = QueryBuilder::<Sqlite>::new(
                "SELECT instance, upvotes, downvotes FROM instance_reputation WHERE instance IN (",
            );
            let mut separated = query.separated(", ");
            for instance in instances {
                separated.push_bind(instance);
            }
            separated.push_unseparated(")");
            let rows = query
                .build()
                .fetch_all(&self.pool)
                .await
                .map_err(|error| StorageError(error.to_string()))?;
            rows.into_iter()
                .map(|row| {
                    let instance: String = row
                        .try_get("instance")
                        .map_err(|error| StorageError(error.to_string()))?;
                    let upvotes: i64 = row
                        .try_get("upvotes")
                        .map_err(|error| StorageError(error.to_string()))?;
                    let downvotes: i64 = row
                        .try_get("downvotes")
                        .map_err(|error| StorageError(error.to_string()))?;
                    Ok((
                        instance,
                        InstanceReputation {
                            upvotes: upvotes.max(0) as u64,
                            downvotes: downvotes.max(0) as u64,
                        },
                    ))
                })
                .collect()
        }

        async fn apply_vote(
            &self,
            instance: &str,
            direction: VoteDirection,
        ) -> Result<InstanceReputation, StorageError> {
            let (upvotes, downvotes) = match direction {
                VoteDirection::Up => (1_i64, 0_i64),
                VoteDirection::Down => (0_i64, 1_i64),
            };
            let row = sqlx::query(
                "INSERT INTO instance_reputation (instance, upvotes, downvotes) VALUES (?, ?, ?) ON CONFLICT(instance) DO UPDATE SET upvotes = upvotes + excluded.upvotes, downvotes = downvotes + excluded.downvotes RETURNING upvotes, downvotes",
            )
            .bind(instance)
            .bind(upvotes)
            .bind(downvotes)
            .fetch_one(&self.pool)
            .await
            .map_err(|error| StorageError(error.to_string()))?;
            let upvotes: i64 = row
                .try_get("upvotes")
                .map_err(|error| StorageError(error.to_string()))?;
            let downvotes: i64 = row
                .try_get("downvotes")
                .map_err(|error| StorageError(error.to_string()))?;
            Ok(InstanceReputation {
                upvotes: upvotes.max(0) as u64,
                downvotes: downvotes.max(0) as u64,
            })
        }
    }

    #[derive(Clone)]
    pub struct RedisStateStore {
        connection: ConnectionManager,
        prefix: String,
    }

    impl std::fmt::Debug for RedisStateStore {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter
                .debug_struct("RedisStateStore")
                .field("prefix", &self.prefix)
                .finish_non_exhaustive()
        }
    }

    impl RedisStateStore {
        pub async fn connect(config: &RedisStorageConfig) -> Result<Self, StorageError> {
            let url = config
                .url
                .as_deref()
                .ok_or_else(|| StorageError("storage.redis.url is required".to_owned()))?;
            let client =
                redis::Client::open(url).map_err(|error| StorageError(error.to_string()))?;
            let mut connection = ConnectionManager::new(client)
                .await
                .map_err(|error| StorageError(error.to_string()))?;
            redis::cmd("PING")
                .query_async::<String>(&mut connection)
                .await
                .map_err(|error| StorageError(error.to_string()))?;
            Ok(Self {
                connection,
                prefix: config.key_prefix.clone(),
            })
        }

        fn key(&self, suffix: &str) -> String {
            format!("{}:{suffix}", self.prefix)
        }
    }

    #[async_trait]
    impl StateStore for RedisStateStore {
        async fn load_crawl_snapshot(&self) -> Result<Option<CrawlSnapshot>, StorageError> {
            let mut connection = self.connection.clone();
            let value = redis::cmd("GET")
                .arg(self.key(CRAWL_SNAPSHOT_KEY))
                .query_async::<Option<String>>(&mut connection)
                .await
                .map_err(|error| StorageError(error.to_string()))?;
            value
                .map(|value| {
                    serde_json::from_str(&value).map_err(|error| StorageError(error.to_string()))
                })
                .transpose()
        }

        async fn save_crawl_snapshot(&self, snapshot: &CrawlSnapshot) -> Result<(), StorageError> {
            let value =
                serde_json::to_string(snapshot).map_err(|error| StorageError(error.to_string()))?;
            let mut connection = self.connection.clone();
            redis::cmd("SET")
                .arg(self.key(CRAWL_SNAPSHOT_KEY))
                .arg(value)
                .query_async::<()>(&mut connection)
                .await
                .map_err(|error| StorageError(error.to_string()))
        }

        async fn get_reputations(
            &self,
            instances: &[String],
        ) -> Result<HashMap<String, InstanceReputation>, StorageError> {
            if instances.is_empty() {
                return Ok(HashMap::new());
            }
            let mut pipeline = redis::pipe();
            pipeline
                .cmd("HMGET")
                .arg(self.key("reputation:up"))
                .arg(instances)
                .cmd("HMGET")
                .arg(self.key("reputation:down"))
                .arg(instances);
            let mut connection = self.connection.clone();
            let (upvotes, downvotes): (Vec<Option<u64>>, Vec<Option<u64>>) = pipeline
                .query_async(&mut connection)
                .await
                .map_err(|error| StorageError(error.to_string()))?;
            Ok(instances
                .iter()
                .enumerate()
                .filter_map(|(index, instance)| {
                    let reputation = InstanceReputation {
                        upvotes: upvotes[index].unwrap_or_default(),
                        downvotes: downvotes[index].unwrap_or_default(),
                    };
                    (reputation != InstanceReputation::default())
                        .then(|| (instance.clone(), reputation))
                })
                .collect())
        }

        async fn apply_vote(
            &self,
            instance: &str,
            direction: VoteDirection,
        ) -> Result<InstanceReputation, StorageError> {
            let script = match direction {
                VoteDirection::Up => {
                    "local up = redis.call('HINCRBY', KEYS[1], ARGV[1], 1); local down = redis.call('HGET', KEYS[2], ARGV[1]) or '0'; return {up, down}"
                }
                VoteDirection::Down => {
                    "local down = redis.call('HINCRBY', KEYS[2], ARGV[1], 1); local up = redis.call('HGET', KEYS[1], ARGV[1]) or '0'; return {up, down}"
                }
            };
            let mut connection = self.connection.clone();
            let values = redis::cmd("EVAL")
                .arg(script)
                .arg(2)
                .arg(self.key("reputation:up"))
                .arg(self.key("reputation:down"))
                .arg(instance)
                .query_async::<Vec<u64>>(&mut connection)
                .await
                .map_err(|error| StorageError(error.to_string()))?;
            Ok(InstanceReputation {
                upvotes: values.first().copied().unwrap_or_default(),
                downvotes: values.get(1).copied().unwrap_or_default(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Arc, time::Duration};

    use chrono::Utc;
    use fastside_shared::config::{RedisStorageConfig, SqliteStorageConfig};

    use super::{
        CrawlSnapshot, InstanceReputation, StateStore, VoteDirection,
        native::{RedisStateStore, SqliteStateStore},
    };
    use crate::crawler::CrawledServices;

    #[test]
    fn bounded_weight_uses_smoothed_ratio() {
        assert_eq!(InstanceReputation::default().weight(0.1, 10.0), 1.0);
        assert_eq!(
            InstanceReputation {
                upvotes: 1,
                downvotes: 0,
            }
            .weight(0.1, 10.0),
            2.0
        );
        assert_eq!(
            InstanceReputation {
                upvotes: 0,
                downvotes: 1,
            }
            .weight(0.1, 10.0),
            0.5
        );
        assert_eq!(
            InstanceReputation {
                upvotes: 10_000,
                downvotes: 0,
            }
            .weight(0.1, 10.0),
            10.0
        );
    }

    async fn storage_conformance(store: Arc<dyn StateStore>) {
        assert_eq!(store.load_crawl_snapshot().await.unwrap(), None);
        let snapshot = CrawlSnapshot {
            crawled_services: CrawledServices {
                services: HashMap::new(),
                time: Utc::now(),
            },
        };
        store.save_crawl_snapshot(&snapshot).await.unwrap();
        assert_eq!(store.load_crawl_snapshot().await.unwrap(), Some(snapshot));

        let instance = "https://reputation.example/".to_owned();
        assert!(
            store
                .get_reputations(std::slice::from_ref(&instance))
                .await
                .unwrap()
                .is_empty()
        );

        let votes = (0..32).map(|index| {
            let store = store.clone();
            let instance = instance.clone();
            tokio::spawn(async move {
                let direction = if index % 4 == 0 {
                    VoteDirection::Down
                } else {
                    VoteDirection::Up
                };
                store.apply_vote(&instance, direction).await.unwrap();
            })
        });
        for vote in votes {
            vote.await.unwrap();
        }

        let reads = (0..8).map(|_| {
            let store = store.clone();
            let instance = instance.clone();
            tokio::spawn(async move {
                store
                    .get_reputations(std::slice::from_ref(&instance))
                    .await
                    .unwrap()[&instance]
            })
        });
        for read in reads {
            assert_eq!(
                read.await.unwrap(),
                InstanceReputation {
                    upvotes: 24,
                    downvotes: 8,
                }
            );
        }
    }

    #[tokio::test]
    async fn sqlite_conforms_to_state_store() {
        let path = std::env::temp_dir().join(format!(
            "fastside-storage-test-{}-{}.sqlite3",
            std::process::id(),
            fastrand::u64(..)
        ));
        let config = SqliteStorageConfig {
            path: path.to_string_lossy().into_owned(),
            max_connections: 8,
            busy_timeout: Duration::from_secs(5),
        };
        let store = Arc::new(SqliteStateStore::connect(&config).await.unwrap());
        storage_conformance(store.clone()).await;
        drop(store);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    #[tokio::test]
    async fn redis_conforms_to_state_store_when_configured() {
        let Ok(url) = std::env::var("FASTSIDE_TEST_REDIS_URL") else {
            return;
        };
        let config = RedisStorageConfig {
            url: Some(url),
            key_prefix: format!("fastside-test-{}", fastrand::u64(..)),
        };
        let store = Arc::new(RedisStateStore::connect(&config).await.unwrap());
        storage_conformance(store).await;
    }
}
