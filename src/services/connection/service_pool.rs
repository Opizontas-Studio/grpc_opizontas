use dashmap::DashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use super::connection::ReverseConnection;

#[derive(Debug, Clone)]
pub(crate) struct ServicePool {
    connections: Arc<DashMap<String, ReverseConnection>>,
    cursor: Arc<AtomicUsize>,
}

impl ServicePool {
    pub(crate) fn new() -> Self {
        Self {
            connections: Arc::new(DashMap::new()),
            cursor: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub(crate) fn add_connection(&self, connection: ReverseConnection) -> Option<ReverseConnection> {
        self.connections
            .insert(connection.connection_id.clone(), connection)
    }

    pub(crate) fn remove_connection(&self, connection_id: &str) -> Option<ReverseConnection> {
        self.connections.remove(connection_id).map(|(_, conn)| conn)
    }

    pub(crate) fn next_connection(&self, timeout: Duration) -> Option<ReverseConnection> {
        let mut active = Vec::new();
        let mut expired_ids = Vec::new();

        for entry in self.connections.iter() {
            let conn = entry.value().clone();
            if conn.is_active && !conn.is_expired(timeout) {
                active.push(conn);
            } else {
                expired_ids.push(entry.key().clone());
            }
        }

        for id in expired_ids {
            self.connections.remove(&id);
        }

        if active.is_empty() {
            return None;
        }

        let idx = self.cursor.fetch_add(1, Ordering::Relaxed);
        Some(active[idx % active.len()].clone())
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.connections.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        self.connections.len()
    }

    pub(crate) fn update_connection<F>(&self, connection_id: &str, mut update_fn: F)
    where
        F: FnMut(&mut ReverseConnection),
    {
        if let Some(mut entry) = self.connections.get_mut(connection_id) {
            update_fn(entry.value_mut());
        }
    }
}