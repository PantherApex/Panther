use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use shared::errors::{PantherError, PantherResult};
use super::types::{CronJob, CronJobState, CronPayload, CronSchedule, compute_next_run, now_ms};

#[derive(Serialize, Deserialize, Default)]
struct CronStore {
    version: u32,
    jobs: Vec<CronJob>,
}

pub type JobHandler = Arc<dyn Fn(CronJob) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync>;

#[derive(Clone)]
pub struct CronService {
    store_path: PathBuf,
    state: Arc<Mutex<CronStore>>,
    handler: Option<JobHandler>,
    timer: Arc<Mutex<Option<JoinHandle<()>>>>,
    running: Arc<std::sync::atomic::AtomicBool>,
}

impl CronService {
    pub fn new(store_path: PathBuf) -> Self {
        Self {
            store_path,
            state: Arc::new(Mutex::new(CronStore::default())),
            handler: None,
            timer: Arc::new(Mutex::new(None)),
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    pub fn with_handler(mut self, handler: JobHandler) -> Self {
        self.handler = Some(handler);
        self
    }

    async fn load(&self) {
        if let Ok(raw) = tokio::fs::read_to_string(&self.store_path).await {
            if let Ok(store) = serde_json::from_str::<CronStore>(&raw) {
                *self.state.lock().await = store;
                return;
            }
        }
        *self.state.lock().await = CronStore::default();
    }

    async fn save(&self) -> PantherResult<()> {
        let state = self.state.lock().await;
        let json = serde_json::to_string_pretty(&*state)
            .map_err(|e| PantherError::ConfigError(e.to_string()))?;
        if let Some(parent) = self.store_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let tmp = self.store_path.with_extension("json.tmp");
        tokio::fs::write(&tmp, &json).await?;
        tokio::fs::rename(&tmp, &self.store_path).await?;
        Ok(())
    }

    pub async fn start(&self) {
        self.load().await;
        {
            let mut state = self.state.lock().await;
            let now = now_ms();
            for job in &mut state.jobs {
                if job.enabled {
                    job.state.next_run_at_ms = compute_next_run(&job.schedule, now);
                }
            }
        }
        let _ = self.save().await;
        self.running.store(true, std::sync::atomic::Ordering::SeqCst);
        self.spawn_timer_loop();
    }

    pub async fn stop(&self) {
        self.running.store(false, std::sync::atomic::Ordering::SeqCst);
        if let Some(handle) = self.timer.lock().await.take() {
            handle.abort();
        }
    }

    fn spawn_timer_loop(&self) {
        let svc = self.clone();
        let handle = tokio::spawn(async move {
            loop {
                if !svc.running.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }

                let next = {
                    let state = svc.state.lock().await;
                    state.jobs.iter()
                        .filter(|j| j.enabled)
                        .filter_map(|j| j.state.next_run_at_ms)
                        .min()
                };

                let delay_ms = match next {
                    Some(t) => (t - now_ms()).max(0) as u64,
                    None => {
                        tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
                        continue;
                    }
                };

                tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;

                if !svc.running.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }

                svc.fire_due_jobs().await;
            }
        });

        let timer = self.timer.clone();
        tokio::spawn(async move {
            *timer.lock().await = Some(handle);
        });
    }

    async fn fire_due_jobs(&self) {
        let now = now_ms();
        let due: Vec<CronJob> = {
            let state = self.state.lock().await;
            state.jobs.iter()
                .filter(|j| j.enabled)
                .filter(|j| j.state.next_run_at_ms.map_or(false, |t| now >= t))
                .cloned()
                .collect()
        };

        for job in due {
            self.execute_job(job).await;
        }

        let _ = self.save().await;
    }

    async fn execute_job(&self, job: CronJob) {
        let job_id = job.id.clone();
        let is_one_shot = matches!(job.schedule, CronSchedule::At { .. });
        let delete_after = job.delete_after_run;

        if let Some(handler) = &self.handler {
            (handler)(job.clone()).await;
        }

        let mut state = self.state.lock().await;
        let now = now_ms();

        if delete_after && is_one_shot {
            state.jobs.retain(|j| j.id != job_id);
            return;
        }

        if let Some(j) = state.jobs.iter_mut().find(|j| j.id == job_id) {
            j.state.last_run_at_ms = Some(now);
            j.state.last_status = Some("ok".to_string());
            j.updated_at_ms = now;
            if is_one_shot {
                j.enabled = false;
                j.state.next_run_at_ms = None;
            } else {
                j.state.next_run_at_ms = compute_next_run(&job.schedule, now);
            }
        }
    }

    pub async fn add_job(
        &self,
        name: String,
        schedule: CronSchedule,
        message: String,
        channel: String,
        chat_id: String,
        delete_after_run: bool,
    ) -> CronJob {
        let now = now_ms();
        let job = CronJob {
            id: Uuid::new_v4().to_string()[..8].to_string(),
            name,
            enabled: true,
            state: CronJobState {
                next_run_at_ms: compute_next_run(&schedule, now),
                ..Default::default()
            },
            schedule,
            payload: CronPayload { message, channel, chat_id },
            created_at_ms: now,
            updated_at_ms: now,
            delete_after_run,
        };

        self.state.lock().await.jobs.push(job.clone());
        let _ = self.save().await;
        job
    }

    pub async fn remove_job(&self, job_id: &str) -> bool {
        let mut state = self.state.lock().await;
        let before = state.jobs.len();
        state.jobs.retain(|j| j.id != job_id);
        let removed = state.jobs.len() < before;
        drop(state);
        if removed {
            let _ = self.save().await;
        }
        removed
    }

    pub async fn list_jobs(&self) -> Vec<CronJob> {
        let state = self.state.lock().await;
        let mut jobs: Vec<CronJob> = state.jobs.iter().filter(|j| j.enabled).cloned().collect();
        jobs.sort_by_key(|j| j.state.next_run_at_ms.unwrap_or(i64::MAX));
        jobs
    }
}
