use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::analyzer::ActivityAnalyzer;
use crate::monitor::ActivityMonitor;
use crate::recorder::{ActivityRecorder, AlertCondition, AlertKind};

pub type AlertFn = Arc<dyn Fn(String) -> Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync>;
pub type AlertSlot = Arc<Mutex<Option<AlertFn>>>;

pub struct ActivityService {
    recorder: Arc<Mutex<ActivityRecorder>>,
    pub analyzer: Arc<ActivityAnalyzer>,
    poll_interval_secs: u64,
    alert_slot: AlertSlot,
    running: Arc<AtomicBool>,
    task: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl ActivityService {
    pub fn new(activity_dir: PathBuf, poll_interval_secs: u64, alert_threshold_mins: u64) -> Self {
        let recorder = ActivityRecorder::new(activity_dir.clone(), alert_threshold_mins);
        let analyzer = ActivityAnalyzer::new(activity_dir);
        Self {
            recorder: Arc::new(Mutex::new(recorder)),
            analyzer: Arc::new(analyzer),
            poll_interval_secs,
            alert_slot: Arc::new(Mutex::new(None)),
            running: Arc::new(AtomicBool::new(false)),
            task: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn set_alert(&self, f: AlertFn) {
        *self.alert_slot.lock().await = Some(f);
    }

    pub async fn start(self: Arc<Self>) {
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let svc = Arc::clone(&self);
        let handle = tokio::spawn(async move {
            svc.run_loop().await;
        });
        *self.task.lock().await = Some(handle);
    }

    pub async fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.task.lock().await.take() {
            handle.abort();
        }
    }

    async fn run_loop(&self) {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(self.poll_interval_secs)).await;
            if !self.running.load(Ordering::SeqCst) {
                break;
            }
            self.tick().await;
        }
    }

    async fn tick(&self) {
        let snapshot = ActivityMonitor::capture().await;
        let alerts = {
            let mut recorder = self.recorder.lock().await;
            recorder.process_snapshot(&snapshot).await
        };

        if !alerts.is_empty() {
            let slot = self.alert_slot.lock().await;
            if let Some(ref alert_fn) = *slot {
                for alert in alerts {
                    let msg = compose_alert_message(&alert);
                    (alert_fn)(msg).await;
                }
            }
        }
    }
}

fn compose_alert_message(alert: &AlertCondition) -> String {
    match alert.kind {
        AlertKind::LongRunningBackgroundProcess => {
            format!(
                "[Activity Monitor] {}{}",
                alert.description,
                if alert.cpu_percent.unwrap_or(0.0) > 5.0 || alert.memory_mb.unwrap_or(0) > 1000 {
                    " This process may be consuming significant resources."
                } else {
                    ""
                }
            )
        }
        AlertKind::DocumentOpenUnused => {
            format!(
                "[Activity Monitor] {}",
                alert.description
            )
        }
    }
}
