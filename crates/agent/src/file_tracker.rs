use std::collections::HashSet;
use std::path::PathBuf;
use tokio::sync::Mutex;

pub struct TurnFileTracker {
    dispatched: Mutex<HashSet<PathBuf>>,
}

impl TurnFileTracker {
    pub fn new() -> Self {
        Self {
            dispatched: Mutex::new(HashSet::new()),
        }
    }

    pub async fn reset(&self) {
        self.dispatched.lock().await.clear();
    }

    pub async fn claim(&self, path: &PathBuf) -> bool {
        self.dispatched.lock().await.insert(path.clone())
    }
}
