use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};

use axum::{extract::{Path as AxumPath, State}, routing::get, Json, Router};
use chrono::{DateTime, Utc};
use napi_derive::napi;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

const DEFAULT_WAL_FILE: &str = "wal.log";
const SNAPSHOT_FILE: &str = "snapshot.bin";
const DEFAULT_MAX_WAL_BYTES: u64 = 100 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobStatus {
    Pending,
    Processing,
    Completed,
    Failed,
    DeadLetter,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    pub id: Uuid,
    pub queue: String,
    pub payload: Vec<u8>,
    pub status: JobStatus,
    pub attempts: u32,
    pub max_attempts: u32,
    pub created_at: DateTime<Utc>,
    pub scheduled_at: Option<DateTime<Utc>>,
}

impl Job {
    pub fn new(queue: impl Into<String>, payload: Vec<u8>, max_attempts: u32) -> Self {
        Self {
            id: Uuid::new_v4(),
            queue: queue.into(),
            payload,
            status: JobStatus::Pending,
            attempts: 0,
            max_attempts,
            created_at: Utc::now(),
            scheduled_at: None,
        }
    }

    pub fn scheduled(
        queue: impl Into<String>,
        payload: Vec<u8>,
        max_attempts: u32,
        scheduled_at: DateTime<Utc>,
    ) -> Self {
        Self {
            scheduled_at: Some(scheduled_at),
            ..Self::new(queue, payload, max_attempts)
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum QueueError {
    #[error("job {0} not found in processing")]
    JobNotProcessing(Uuid),
}

#[derive(Debug, Error)]
pub enum WalError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] bincode::Error),
    #[error("record too large to store in wal: {0} bytes")]
    RecordTooLarge(usize),
}

#[derive(Debug, Error)]
pub enum PersistentQueueError {
    #[error(transparent)]
    Queue(#[from] QueueError),
    #[error(transparent)]
    Wal(#[from] WalError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WalEvent {
    Enqueue(Job),
    Dequeue(Uuid),
    Ack(Uuid),
    Nack(Uuid, String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
struct QueueSnapshot {
    pending: VecDeque<Job>,
    processing: HashMap<Uuid, Job>,
    scheduled: Vec<Job>,
    completed: Vec<Job>,
    dead_letter: Vec<Job>,
    failure_reasons: HashMap<Uuid, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ScheduledEntry {
    scheduled_at: DateTime<Utc>,
    job_id: Uuid,
}

#[derive(Debug, Default)]
pub struct Queue {
    pub pending: VecDeque<Job>,
    pub processing: HashMap<Uuid, Job>,
    scheduled: HashMap<Uuid, Job>,
    scheduled_heap: BinaryHeap<Reverse<ScheduledEntry>>,
    completed: Vec<Job>,
    dead_letter: Vec<Job>,
    failure_reasons: HashMap<Uuid, String>,
}

impl Queue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enqueue(&mut self, job: Job) {
        self.apply_enqueue(job);
    }

    pub fn dequeue(&mut self) -> Option<Job> {
        let job = self.pending.pop_front()?;
        Some(self.move_pending_job_to_processing(job))
    }

    pub fn dequeue_for_queue(&mut self, queue_name: &str) -> Option<Job> {
        let index = self.pending.iter().position(|job| job.queue == queue_name)?;
        let job = self.pending.remove(index)?;
        Some(self.move_pending_job_to_processing(job))
    }

    pub fn ack(&mut self, id: Uuid) -> Result<Job, QueueError> {
        self.apply_ack(id)
    }

    pub fn nack(&mut self, id: Uuid, reason: impl Into<String>) -> Result<Job, QueueError> {
        self.apply_nack(id, reason.into())
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    pub fn processing_len(&self) -> usize {
        self.processing.len()
    }

    pub fn scheduled_len(&self) -> usize {
        self.scheduled.len()
    }

    pub fn completed_len(&self) -> usize {
        self.completed.len()
    }

    pub fn dead_letter_len(&self) -> usize {
        self.dead_letter.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty() && self.processing.is_empty() && self.scheduled.is_empty()
    }

    pub fn completed_jobs(&self) -> &[Job] {
        &self.completed
    }

    pub fn dead_letter_jobs(&self) -> &[Job] {
        &self.dead_letter
    }

    pub fn failure_reason(&self, id: Uuid) -> Option<&str> {
        self.failure_reasons.get(&id).map(String::as_str)
    }

    pub fn promote_ready_jobs(&mut self, now: DateTime<Utc>) -> usize {
        let mut promoted = 0usize;

        while let Some(Reverse(entry)) = self.scheduled_heap.peek().cloned() {
            if entry.scheduled_at > now {
                break;
            }

            self.scheduled_heap.pop();
            if let Some(mut job) = self.scheduled.remove(&entry.job_id) {
                job.scheduled_at = Some(entry.scheduled_at);
                self.pending.push_back(job);
                promoted += 1;
            }
        }

        promoted
    }

    fn apply_event(&mut self, event: WalEvent) -> Result<(), QueueError> {
        match event {
            WalEvent::Enqueue(job) => {
                self.apply_enqueue(job);
                Ok(())
            }
            WalEvent::Dequeue(id) => {
                let _ = self.apply_dequeue(id);
                Ok(())
            }
            WalEvent::Ack(id) => {
                let _ = self.apply_ack(id)?;
                Ok(())
            }
            WalEvent::Nack(id, reason) => {
                let _ = self.apply_nack(id, reason)?;
                Ok(())
            }
        }
    }

    fn apply_enqueue(&mut self, mut job: Job) {
        job.status = JobStatus::Pending;
        if let Some(scheduled_at) = job.scheduled_at {
            if scheduled_at > Utc::now() {
                self.scheduled_heap.push(Reverse(ScheduledEntry {
                    scheduled_at,
                    job_id: job.id,
                }));
                self.scheduled.insert(job.id, job);
                return;
            }
        }
        self.pending.push_back(job);
    }

    fn move_pending_job_to_processing(&mut self, mut job: Job) -> Job {
        job.status = JobStatus::Processing;
        self.processing.insert(job.id, job.clone());
        job
    }

    fn apply_dequeue(&mut self, id: Uuid) -> Option<Job> {
        let index = self.pending.iter().position(|job| job.id == id)?;
        let job = self.pending.remove(index)?;
        Some(self.move_pending_job_to_processing(job))
    }

    fn apply_ack(&mut self, id: Uuid) -> Result<Job, QueueError> {
        let mut job = self
            .processing
            .remove(&id)
            .ok_or(QueueError::JobNotProcessing(id))?;
        job.status = JobStatus::Completed;
        self.failure_reasons.remove(&id);
        self.completed.push(job.clone());
        Ok(job)
    }

    fn apply_nack(&mut self, id: Uuid, reason: String) -> Result<Job, QueueError> {
        let mut job = self
            .processing
            .remove(&id)
            .ok_or(QueueError::JobNotProcessing(id))?;

        job.attempts = job.attempts.saturating_add(1);
        self.failure_reasons.insert(id, reason);

        if job.attempts >= job.max_attempts {
            job.status = JobStatus::DeadLetter;
            self.dead_letter.push(job.clone());
            Ok(job)
        } else {
            job.status = JobStatus::Pending;
            self.pending.push_back(job.clone());
            Ok(job)
        }
    }

    fn snapshot(&self) -> QueueSnapshot {
        QueueSnapshot {
            pending: self.pending.clone(),
            processing: self.processing.clone(),
            scheduled: self.scheduled.values().cloned().collect(),
            completed: self.completed.clone(),
            dead_letter: self.dead_letter.clone(),
            failure_reasons: self.failure_reasons.clone(),
        }
    }

    fn from_snapshot(snapshot: QueueSnapshot) -> Self {
        let mut queue = Self {
            pending: snapshot.pending,
            processing: snapshot.processing,
            scheduled: HashMap::new(),
            scheduled_heap: BinaryHeap::new(),
            completed: snapshot.completed,
            dead_letter: snapshot.dead_letter,
            failure_reasons: snapshot.failure_reasons,
        };

        for job in snapshot.scheduled {
            if let Some(scheduled_at) = job.scheduled_at {
                queue.scheduled_heap.push(Reverse(ScheduledEntry {
                    scheduled_at,
                    job_id: job.id,
                }));
            }
            queue.scheduled.insert(job.id, job);
        }

        queue
    }
}

#[derive(Debug)]
struct Wal {
    dir: PathBuf,
    active_path: PathBuf,
    max_bytes: u64,
}

impl Wal {
    fn open(dir: impl AsRef<Path>, max_bytes: u64) -> Result<Self, WalError> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        let active_path = dir.join(DEFAULT_WAL_FILE);
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&active_path)?;

        Ok(Self {
            dir,
            active_path,
            max_bytes,
        })
    }

    fn append(&mut self, event: &WalEvent) -> Result<(), WalError> {
        let payload = bincode::serialize(event)?;
        let payload_len =
            u32::try_from(payload.len()).map_err(|_| WalError::RecordTooLarge(payload.len()))?;

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.active_path)?;
        file.write_all(&payload_len.to_le_bytes())?;
        file.write_all(&payload)?;
        file.sync_all()?;
        Ok(())
    }

    fn load_queue(&self) -> Result<Queue, WalError> {
        let mut queue = self.read_snapshot()?;
        for path in self.segment_paths()? {
            Self::replay_file_into_queue(&path, &mut queue)?;
        }
        Ok(queue)
    }

    fn rotate_and_compact(&mut self, queue: &Queue) -> Result<(), WalError> {
        let active_len = fs::metadata(&self.active_path)?.len();
        if active_len <= self.max_bytes {
            return Ok(());
        }

        let rotated_path = self.next_segment_path();
        if rotated_path.exists() {
            fs::remove_file(&rotated_path)?;
        }

        fs::rename(&self.active_path, &rotated_path)?;
        self.write_snapshot(queue)?;
        self.delete_old_segments()?;
        File::create(&self.active_path)?.sync_all()?;
        Ok(())
    }

    fn replay_file_into_queue(path: &Path, queue: &mut Queue) -> Result<(), WalError> {
        if !path.exists() {
            return Ok(());
        }

        let file = File::open(path)?;
        let mut reader = BufReader::new(file);

        loop {
            let mut len_buf = [0u8; 4];
            match reader.read_exact(&mut len_buf) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(err) => return Err(WalError::Io(err)),
            }

            let record_len = u32::from_le_bytes(len_buf) as usize;
            let mut payload = vec![0u8; record_len];
            match reader.read_exact(&mut payload) {
                Ok(()) => {
                    let event: WalEvent = bincode::deserialize(&payload)?;
                    if let Err(_err) = queue.apply_event(event) {
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(err) => return Err(WalError::Io(err)),
            }
        }

        Ok(())
    }

    fn write_snapshot(&self, queue: &Queue) -> Result<(), WalError> {
        let bytes = bincode::serialize(&queue.snapshot())?;
        let snapshot_path = self.dir.join(SNAPSHOT_FILE);
        let mut file = File::create(snapshot_path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        Ok(())
    }

    fn read_snapshot(&self) -> Result<Queue, WalError> {
        let snapshot_path = self.dir.join(SNAPSHOT_FILE);
        if !snapshot_path.exists() {
            return Ok(Queue::new());
        }

        let bytes = fs::read(snapshot_path)?;
        let snapshot: QueueSnapshot = bincode::deserialize(&bytes)?;
        Ok(Queue::from_snapshot(snapshot))
    }

    fn segment_paths(&self) -> Result<Vec<PathBuf>, WalError> {
        let mut segments = fs::read_dir(&self.dir)?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name == DEFAULT_WAL_FILE || Self::is_rotated_segment(name))
            })
            .collect::<Vec<_>>();

        segments.sort_by(|left, right| {
            segment_sort_key(left.as_path()).cmp(&segment_sort_key(right.as_path()))
        });

        Ok(segments)
    }

    fn next_segment_path(&self) -> PathBuf {
        let mut highest = 0u32;
        if let Ok(read_dir) = fs::read_dir(&self.dir) {
            for entry in read_dir.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    if let Some(index) = parse_segment_index(name) {
                        highest = highest.max(index);
                    }
                }
            }
        }

        self.dir.join(format!("wal.{:03}.log", highest + 1))
    }

    fn delete_old_segments(&self) -> Result<(), WalError> {
        for path in self.segment_paths()? {
            if path != self.active_path {
                fs::remove_file(path)?;
            }
        }
        Ok(())
    }

    fn is_rotated_segment(name: &str) -> bool {
        parse_segment_index(name).is_some()
    }
}

fn parse_segment_index(name: &str) -> Option<u32> {
    name.strip_prefix("wal.")
        .and_then(|value| value.strip_suffix(".log"))
        .and_then(|value| value.parse::<u32>().ok())
}

fn segment_sort_key(path: &Path) -> (u32, String) {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string();

    if name == DEFAULT_WAL_FILE {
        (u32::MAX, name)
    } else {
        (parse_segment_index(&name).unwrap_or_default(), name)
    }
}

#[derive(Debug)]
pub struct PersistentQueue {
    queue: Queue,
    wal: Wal,
}

impl PersistentQueue {
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, WalError> {
        Self::open_with_max_bytes(dir, DEFAULT_MAX_WAL_BYTES)
    }

    pub fn open_with_max_bytes(dir: impl AsRef<Path>, max_bytes: u64) -> Result<Self, WalError> {
        let wal = Wal::open(dir, max_bytes)?;
        let queue = wal.load_queue()?;
        Ok(Self { queue, wal })
    }

    pub fn queue(&self) -> &Queue {
        &self.queue
    }

    pub fn queue_mut(&mut self) -> &mut Queue {
        &mut self.queue
    }

    pub fn enqueue(&mut self, job: Job) -> Result<(), PersistentQueueError> {
        let event = WalEvent::Enqueue(job);
        self.wal.append(&event)?;
        self.queue.apply_event(event)?;
        self.wal.rotate_and_compact(&self.queue)?;
        Ok(())
    }

    pub fn dequeue(&mut self) -> Result<Option<Job>, PersistentQueueError> {
        let Some(job) = self.queue.pending.front().cloned() else {
            return Ok(None);
        };

        let event = WalEvent::Dequeue(job.id);
        self.wal.append(&event)?;
        self.queue.apply_event(event)?;
        self.wal.rotate_and_compact(&self.queue)?;
        Ok(self.queue.processing.get(&job.id).cloned())
    }

    pub fn dequeue_for_queue(
        &mut self,
        queue_name: &str,
    ) -> Result<Option<Job>, PersistentQueueError> {
        let Some(job) = self
            .queue
            .pending
            .iter()
            .find(|job| job.queue == queue_name)
            .cloned()
        else {
            return Ok(None);
        };

        let event = WalEvent::Dequeue(job.id);
        self.wal.append(&event)?;
        self.queue.apply_event(event)?;
        self.wal.rotate_and_compact(&self.queue)?;
        Ok(self.queue.processing.get(&job.id).cloned())
    }

    pub fn ack(&mut self, id: Uuid) -> Result<Job, PersistentQueueError> {
        let event = WalEvent::Ack(id);
        self.wal.append(&event)?;
        let job = self.queue.apply_ack(id)?;
        self.wal.rotate_and_compact(&self.queue)?;
        Ok(job)
    }

    pub fn nack(&mut self, id: Uuid, reason: impl Into<String>) -> Result<Job, PersistentQueueError> {
        let reason = reason.into();
        let event = WalEvent::Nack(id, reason.clone());
        self.wal.append(&event)?;
        let job = self.queue.apply_nack(id, reason)?;
        self.wal.rotate_and_compact(&self.queue)?;
        Ok(job)
    }

    pub fn compact(&mut self) -> Result<(), WalError> {
        self.wal.write_snapshot(&self.queue)?;
        self.wal.delete_old_segments()?;
        File::create(&self.wal.active_path)?.sync_all()?;
        Ok(())
    }
}

#[derive(Debug)]
struct SharedQueueHandle {
    queue: std::sync::Mutex<Queue>,
}

#[derive(Clone)]
#[napi(object)]
pub struct JsQueueJob {
    pub id: String,
    pub queue: String,
    pub payload_json: String,
    pub status: String,
    pub attempts: u32,
    pub max_attempts: u32,
    pub created_at: String,
    pub scheduled_at: Option<String>,
}

impl From<Job> for JsQueueJob {
    fn from(value: Job) -> Self {
        Self {
            id: value.id.to_string(),
            queue: value.queue,
            payload_json: String::from_utf8_lossy(&value.payload).to_string(),
            status: format!("{:?}", value.status).to_lowercase(),
            attempts: value.attempts,
            max_attempts: value.max_attempts,
            created_at: value.created_at.to_rfc3339(),
            scheduled_at: value.scheduled_at.map(|value| value.to_rfc3339()),
        }
    }
}

#[derive(Clone)]
#[napi(object)]
pub struct JsQueueStats {
    pub pending: u32,
    pub processing: u32,
    pub scheduled: u32,
    pub completed: u32,
    pub dead_letter: u32,
}

#[napi]
pub struct JsQueue {
    inner: Arc<SharedQueueHandle>,
}

#[napi]
impl JsQueue {
    #[napi(constructor)]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(SharedQueueHandle {
                queue: std::sync::Mutex::new(Queue::new()),
            }),
        }
    }

    #[napi]
    pub async fn enqueue(
        &self,
        queue: String,
        payload_json: String,
        max_attempts: Option<u32>,
    ) -> napi::Result<JsQueueJob> {
        let job = Job::new(queue, payload_json.into_bytes(), max_attempts.unwrap_or(3));
        let mut guard = self.lock_queue()?;
        guard.enqueue(job.clone());
        Ok(job.into())
    }

    #[napi]
    pub async fn schedule(
        &self,
        queue: String,
        payload_json: String,
        run_at_rfc3339: String,
        max_attempts: Option<u32>,
    ) -> napi::Result<JsQueueJob> {
        let run_at = DateTime::parse_from_rfc3339(&run_at_rfc3339)
            .map_err(|err| napi::Error::from_reason(err.to_string()))?
            .with_timezone(&Utc);
        let job = Job::scheduled(queue, payload_json.into_bytes(), max_attempts.unwrap_or(3), run_at);
        let mut guard = self.lock_queue()?;
        guard.enqueue(job.clone());
        Ok(job.into())
    }

    #[napi]
    pub async fn dequeue(&self, queue: Option<String>) -> napi::Result<Option<JsQueueJob>> {
        let mut guard = self.lock_queue()?;
        let job = match queue {
            Some(queue_name) => guard.dequeue_for_queue(&queue_name),
            None => guard.dequeue(),
        };
        Ok(job.map(Into::into))
    }

    #[napi]
    pub async fn ack(&self, id: String) -> napi::Result<JsQueueJob> {
        let id = parse_uuid(&id)?;
        let mut guard = self.lock_queue()?;
        guard
            .ack(id)
            .map(Into::into)
            .map_err(|err| napi::Error::from_reason(err.to_string()))
    }

    #[napi]
    pub async fn nack(&self, id: String, reason: String) -> napi::Result<JsQueueJob> {
        let id = parse_uuid(&id)?;
        let mut guard = self.lock_queue()?;
        guard
            .nack(id, reason)
            .map(Into::into)
            .map_err(|err| napi::Error::from_reason(err.to_string()))
    }

    #[napi]
    pub async fn stats(&self) -> napi::Result<JsQueueStats> {
        let mut guard = self.lock_queue()?;
        let _ = guard.promote_ready_jobs(Utc::now());
        Ok(JsQueueStats {
            pending: guard.pending_len() as u32,
            processing: guard.processing_len() as u32,
            scheduled: guard.scheduled_len() as u32,
            completed: guard.completed_len() as u32,
            dead_letter: guard.dead_letter_len() as u32,
        })
    }

    #[napi]
    pub fn create_worker(&self, queue: Option<String>) -> JsWorker {
        JsWorker {
            inner: Arc::clone(&self.inner),
            queue,
        }
    }

    fn lock_queue(&self) -> napi::Result<std::sync::MutexGuard<'_, Queue>> {
        self.inner
            .queue
            .lock()
            .map_err(|_| napi::Error::from_reason("queue mutex poisoned".to_string()))
    }
}

impl Default for JsQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[napi]
pub struct JsWorker {
    inner: Arc<SharedQueueHandle>,
    queue: Option<String>,
}

#[napi]
impl JsWorker {
    #[napi]
    pub async fn poll_once(&self) -> napi::Result<Option<JsQueueJob>> {
        let mut guard = self
            .inner
            .queue
            .lock()
            .map_err(|_| napi::Error::from_reason("queue mutex poisoned".to_string()))?;
        let job = match &self.queue {
            Some(queue_name) => guard.dequeue_for_queue(queue_name),
            None => guard.dequeue(),
        };
        Ok(job.map(Into::into))
    }

    #[napi]
    pub async fn ack(&self, id: String) -> napi::Result<JsQueueJob> {
        let id = parse_uuid(&id)?;
        let mut guard = self
            .inner
            .queue
            .lock()
            .map_err(|_| napi::Error::from_reason("queue mutex poisoned".to_string()))?;
        guard
            .ack(id)
            .map(Into::into)
            .map_err(|err| napi::Error::from_reason(err.to_string()))
    }

    #[napi]
    pub async fn nack(&self, id: String, reason: String) -> napi::Result<JsQueueJob> {
        let id = parse_uuid(&id)?;
        let mut guard = self
            .inner
            .queue
            .lock()
            .map_err(|_| napi::Error::from_reason("queue mutex poisoned".to_string()))?;
        guard
            .nack(id, reason)
            .map(Into::into)
            .map_err(|err| napi::Error::from_reason(err.to_string()))
    }
}

#[napi]
pub fn create_queue() -> JsQueue {
    JsQueue::new()
}

fn parse_uuid(value: &str) -> napi::Result<Uuid> {
    Uuid::parse_str(value).map_err(|err| napi::Error::from_reason(err.to_string()))
}

#[derive(Debug, Clone)]
pub struct QueueWsConfig {
    pub host: String,
    pub port: u16,
}

impl Default for QueueWsConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 6380,
        }
    }
}

impl QueueWsConfig {
    pub fn addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum QueueRequest {
    Enqueue {
        queue: String,
        payload: serde_json::Value,
        #[serde(default = "default_max_attempts")]
        max_attempts: u32,
    },
    Schedule {
        queue: String,
        payload: serde_json::Value,
        run_at: DateTime<Utc>,
        #[serde(default = "default_max_attempts")]
        max_attempts: u32,
    },
    Cron {
        queue: String,
        payload: serde_json::Value,
        expression: String,
        #[serde(default = "default_max_attempts")]
        max_attempts: u32,
    },
    Dequeue {
        queue: Option<String>,
    },
    Ack {
        id: Uuid,
    },
    Heartbeat {
        id: Uuid,
    },
    Nack {
        id: Uuid,
        reason: String,
    },
    Subscribe {
        queue: String,
    },
    Stats,
}

fn default_max_attempts() -> u32 {
    3
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QueueResponse {
    Enqueued {
        job: Job,
    },
    Dequeued {
        job: Option<Job>,
    },
    Acked {
        job: Job,
    },
    HeartbeatRecorded {
        id: Uuid,
    },
    Nacked {
        job: Job,
    },
    Subscribed {
        queue: String,
    },
    Stats {
        pending: usize,
        processing: usize,
        scheduled: usize,
        completed: usize,
        dead_letter: usize,
    },
    QueueStats {
        stats: QueueStatsView,
    },
    CronRegistered {
        queue: String,
        expression: String,
    },
    Event {
        queue: String,
        kind: String,
        job: Job,
    },
    Error {
        message: String,
    },
}

type SharedPersistentQueue = std::sync::Arc<tokio::sync::Mutex<PersistentQueue>>;
type SubscriptionMap = std::sync::Arc<
    tokio::sync::Mutex<HashMap<String, tokio::sync::broadcast::Sender<QueueResponse>>>,
>;
type RecurringJobStore = std::sync::Arc<tokio::sync::Mutex<Vec<RecurringJob>>>;
type QueueRuntimeState = Arc<QueueRuntime>;

#[derive(Debug, Clone)]
struct RecurringJob {
    queue: String,
    payload: serde_json::Value,
    max_attempts: u32,
    schedule: cron::Schedule,
    next_run_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct WorkerPoolConfig {
    pub default_concurrency: usize,
    pub heartbeat_interval: StdDuration,
    pub heartbeat_timeout: StdDuration,
}

impl Default for WorkerPoolConfig {
    fn default() -> Self {
        Self {
            default_concurrency: 1,
            heartbeat_interval: StdDuration::from_secs(30),
            heartbeat_timeout: StdDuration::from_secs(60),
        }
    }
}

#[derive(Debug)]
struct ActiveJob {
    queue: String,
    last_heartbeat: Instant,
    _permit: OwnedSemaphorePermit,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QueueStatsView {
    pub queue: String,
    pub pending: usize,
    pub processing: usize,
    pub failed: usize,
    pub throughput_per_sec: f64,
}

#[derive(Debug)]
struct QueueRuntime {
    config: WorkerPoolConfig,
    semaphores: Mutex<HashMap<String, Arc<Semaphore>>>,
    active_jobs: Mutex<HashMap<Uuid, ActiveJob>>,
    completions: Mutex<VecDeque<(Instant, String)>>,
}

impl QueueRuntime {
    fn new(config: WorkerPoolConfig) -> Self {
        Self {
            config,
            semaphores: Mutex::new(HashMap::new()),
            active_jobs: Mutex::new(HashMap::new()),
            completions: Mutex::new(VecDeque::new()),
        }
    }

    async fn set_concurrency(&self, queue: &str, concurrency: usize) {
        let limit = concurrency.max(1);
        let mut guard = self.semaphores.lock().await;
        guard.insert(queue.to_string(), Arc::new(Semaphore::new(limit)));
    }

    async fn acquire_permit(
        &self,
        queue: &str,
    ) -> Result<Option<OwnedSemaphorePermit>, PersistentQueueError> {
        let semaphore = {
            let mut guard = self.semaphores.lock().await;
            Arc::clone(
                guard
                    .entry(queue.to_string())
                    .or_insert_with(|| Arc::new(Semaphore::new(self.config.default_concurrency))),
            )
        };

        match semaphore.try_acquire_owned() {
            Ok(permit) => Ok(Some(permit)),
            Err(tokio::sync::TryAcquireError::NoPermits) => Ok(None),
            Err(tokio::sync::TryAcquireError::Closed) => Err(PersistentQueueError::Wal(
                WalError::Io(std::io::Error::other("semaphore closed")),
            )),
        }
    }

    async fn register_processing(
        &self,
        queue: &str,
        job_id: Uuid,
        permit: OwnedSemaphorePermit,
    ) {
        self.active_jobs.lock().await.insert(
            job_id,
            ActiveJob {
                queue: queue.to_string(),
                last_heartbeat: Instant::now(),
                _permit: permit,
            },
        );
    }

    async fn heartbeat(&self, job_id: Uuid) -> bool {
        if let Some(active) = self.active_jobs.lock().await.get_mut(&job_id) {
            active.last_heartbeat = Instant::now();
            true
        } else {
            false
        }
    }

    async fn finish_job(&self, job_id: Uuid, record_completion: bool) {
        let removed = self.active_jobs.lock().await.remove(&job_id);
        if record_completion {
            if let Some(active) = removed {
                let mut completions = self.completions.lock().await;
                completions.push_back((Instant::now(), active.queue));
                prune_completions(&mut completions);
            }
        }
    }

    async fn stats_for_queue(&self, queue: &Queue, queue_name: &str) -> QueueStatsView {
        let now = Instant::now();
        let mut completions = self.completions.lock().await;
        prune_completions(&mut completions);
        let completed_in_window = completions
            .iter()
            .filter(|(ts, name)| name == queue_name && now.duration_since(*ts) <= StdDuration::from_secs(1))
            .count();

        QueueStatsView {
            queue: queue_name.to_string(),
            pending: queue.pending.iter().filter(|job| job.queue == queue_name).count(),
            processing: queue.processing.values().filter(|job| job.queue == queue_name).count(),
            failed: queue
                .dead_letter_jobs()
                .iter()
                .filter(|job| job.queue == queue_name)
                .count(),
            throughput_per_sec: completed_in_window as f64,
        }
    }
}

fn prune_completions(completions: &mut VecDeque<(Instant, String)>) {
    let now = Instant::now();
    while completions
        .front()
        .is_some_and(|(ts, _)| now.duration_since(*ts) > StdDuration::from_secs(1))
    {
        completions.pop_front();
    }
}

pub struct QueueWebSocketServer {
    queue: SharedPersistentQueue,
    config: QueueWsConfig,
    subscriptions: SubscriptionMap,
    recurring_jobs: RecurringJobStore,
    runtime: QueueRuntimeState,
}

impl QueueWebSocketServer {
    pub fn new(queue: PersistentQueue, config: QueueWsConfig) -> Self {
        Self {
            queue: std::sync::Arc::new(tokio::sync::Mutex::new(queue)),
            config,
            subscriptions: std::sync::Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            recurring_jobs: std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new())),
            runtime: Arc::new(QueueRuntime::new(WorkerPoolConfig::default())),
        }
    }

    pub fn with_worker_pool_config(mut self, config: WorkerPoolConfig) -> Self {
        self.runtime = Arc::new(QueueRuntime::new(config));
        self
    }

    pub async fn set_queue_concurrency(&self, queue: &str, concurrency: usize) {
        self.runtime.set_concurrency(queue, concurrency).await;
    }

    pub fn stats_router(&self) -> Router {
        Router::new()
            .route("/queues/:name/stats", get(get_queue_stats))
            .with_state((Arc::clone(&self.queue), Arc::clone(&self.runtime)))
    }

    pub async fn bind(self) -> Result<(), WalError> {
        use futures_util::{SinkExt, StreamExt};
        use tokio::net::TcpListener;
        use tokio_tungstenite::{accept_async, tungstenite::Message};

        let listener = TcpListener::bind(self.config.addr()).await?;
        let queue = std::sync::Arc::clone(&self.queue);
        let subscriptions = std::sync::Arc::clone(&self.subscriptions);
        let recurring_jobs = std::sync::Arc::clone(&self.recurring_jobs);
        let runtime = Arc::clone(&self.runtime);

        tokio::spawn(run_scheduled_job_promoter(
            std::sync::Arc::clone(&queue),
            std::sync::Arc::clone(&subscriptions),
        ));
        tokio::spawn(run_cron_scheduler(
            std::sync::Arc::clone(&queue),
            std::sync::Arc::clone(&subscriptions),
            std::sync::Arc::clone(&recurring_jobs),
        ));
        tokio::spawn(run_heartbeat_monitor(
            std::sync::Arc::clone(&queue),
            std::sync::Arc::clone(&runtime),
        ));

        loop {
            let (stream, _) = listener.accept().await?;
            let queue = std::sync::Arc::clone(&queue);
            let subscriptions = std::sync::Arc::clone(&subscriptions);
            let recurring_jobs = std::sync::Arc::clone(&recurring_jobs);
            let runtime = Arc::clone(&runtime);

            tokio::spawn(async move {
                let ws_stream = match accept_async(stream).await {
                    Ok(stream) => stream,
                    Err(_) => return,
                };
                let (mut writer, mut reader) = ws_stream.split();
                let mut subscriptions_rx: Vec<tokio::sync::broadcast::Receiver<QueueResponse>> =
                    Vec::new();

                loop {
                    tokio::select! {
                        maybe_message = reader.next() => {
                            let message = match maybe_message {
                                Some(Ok(Message::Text(text))) => text,
                                Some(Ok(Message::Binary(binary))) => String::from_utf8_lossy(&binary).to_string(),
                                Some(Ok(Message::Close(_))) | None => break,
                                Some(Ok(_)) => continue,
                                Some(Err(_)) => break,
                            };

                            let response = handle_queue_request(
                                &queue,
                                &subscriptions,
                                &recurring_jobs,
                                &runtime,
                                &mut subscriptions_rx,
                                &message,
                            ).await;

                            let serialized = match serde_json::to_string(&response) {
                                Ok(serialized) => serialized,
                                Err(_) => break,
                            };
                            if writer.send(Message::Text(serialized)).await.is_err() {
                                break;
                            }
                        }
                        event = next_subscription_event(&mut subscriptions_rx), if !subscriptions_rx.is_empty() => {
                            match event {
                                Some(event) => {
                                    let serialized = match serde_json::to_string(&event) {
                                        Ok(serialized) => serialized,
                                        Err(_) => break,
                                    };
                                    if writer.send(Message::Text(serialized)).await.is_err() {
                                        break;
                                    }
                                }
                                None => tokio::task::yield_now().await,
                            }
                        }
                    }
                }
            });
        }
    }
}

async fn subscription_sender(
    subscriptions: &SubscriptionMap,
    queue: &str,
) -> tokio::sync::broadcast::Sender<QueueResponse> {
    let mut guard = subscriptions.lock().await;
    if let Some(sender) = guard.get(queue) {
        return sender.clone();
    }
    let (sender, _) = tokio::sync::broadcast::channel(1024);
    guard.insert(queue.to_string(), sender.clone());
    sender
}

async fn broadcast_job_event(
    subscriptions: &SubscriptionMap,
    queue: String,
    kind: &str,
    job: Job,
) {
    let sender = subscription_sender(subscriptions, &queue).await;
    let _ = sender.send(QueueResponse::Event {
        queue,
        kind: kind.to_string(),
        job,
    });
}

async fn handle_queue_request(
    queue: &SharedPersistentQueue,
    subscriptions: &SubscriptionMap,
    recurring_jobs: &RecurringJobStore,
    runtime: &QueueRuntimeState,
    subscriptions_rx: &mut Vec<tokio::sync::broadcast::Receiver<QueueResponse>>,
    message: &str,
) -> QueueResponse {
    match serde_json::from_str::<QueueRequest>(message) {
        Ok(QueueRequest::Enqueue {
            queue: queue_name,
            payload,
            max_attempts,
        }) => {
            if matches!(payload, serde_json::Value::Null) {
                return QueueResponse::Error {
                    message: "payload cannot be null".to_string(),
                };
            }

            let payload_bytes = match serde_json::to_vec(&payload) {
                Ok(payload_bytes) => payload_bytes,
                Err(err) => {
                    return QueueResponse::Error {
                        message: format!("invalid payload: {}", err),
                    }
                }
            };

            let mut guard = queue.lock().await;
            let job = Job::new(queue_name.clone(), payload_bytes, max_attempts);
            match guard.enqueue(job.clone()) {
                Ok(()) => {
                    drop(guard);
                    broadcast_job_event(subscriptions, queue_name, "enqueued", job.clone()).await;
                    QueueResponse::Enqueued { job }
                }
                Err(err) => QueueResponse::Error {
                    message: err.to_string(),
                },
            }
        }
        Ok(QueueRequest::Schedule {
            queue: queue_name,
            payload,
            run_at,
            max_attempts,
        }) => {
            if matches!(payload, serde_json::Value::Null) {
                return QueueResponse::Error {
                    message: "payload cannot be null".to_string(),
                };
            }

            let payload_bytes = match serde_json::to_vec(&payload) {
                Ok(payload_bytes) => payload_bytes,
                Err(err) => {
                    return QueueResponse::Error {
                        message: format!("invalid payload: {}", err),
                    }
                }
            };

            let mut guard = queue.lock().await;
            let job = Job::scheduled(queue_name.clone(), payload_bytes, max_attempts, run_at);
            match guard.enqueue(job.clone()) {
                Ok(()) => QueueResponse::Enqueued { job },
                Err(err) => QueueResponse::Error {
                    message: err.to_string(),
                },
            }
        }
        Ok(QueueRequest::Cron {
            queue: queue_name,
            payload,
            expression,
            max_attempts,
        }) => {
            let schedule = match expression.parse::<cron::Schedule>() {
                Ok(schedule) => schedule,
                Err(err) => {
                    return QueueResponse::Error {
                        message: format!("invalid cron expression: {}", err),
                    }
                }
            };

            let next_run_at = match schedule.upcoming(Utc).next() {
                Some(next_run_at) => next_run_at,
                None => {
                    return QueueResponse::Error {
                        message: "cron expression has no future run time".to_string(),
                    }
                }
            };

            recurring_jobs.lock().await.push(RecurringJob {
                queue: queue_name.clone(),
                payload,
                max_attempts,
                schedule,
                next_run_at,
            });

            QueueResponse::CronRegistered {
                queue: queue_name,
                expression,
            }
        }
        Ok(QueueRequest::Dequeue { queue: queue_name }) => {
            let mut guard = queue.lock().await;
            let result = match queue_name.as_deref() {
                Some(queue_name) => {
                    let permit = match runtime.acquire_permit(queue_name).await {
                        Ok(Some(permit)) => permit,
                        Ok(None) => return QueueResponse::Dequeued { job: None },
                        Err(err) => {
                            return QueueResponse::Error {
                                message: err.to_string(),
                            }
                        }
                    };
                    match guard.dequeue_for_queue(queue_name) {
                        Ok(Some(job)) => {
                            runtime.register_processing(queue_name, job.id, permit).await;
                            Ok(Some(job))
                        }
                        Ok(None) => Ok(None),
                        Err(err) => Err(err),
                    }
                }
                None => guard.dequeue(),
            };
            match result {
                Ok(job) => QueueResponse::Dequeued { job },
                Err(err) => QueueResponse::Error {
                    message: err.to_string(),
                },
            }
        }
        Ok(QueueRequest::Ack { id }) => {
            let mut guard = queue.lock().await;
            match guard.ack(id) {
                Ok(job) => {
                    drop(guard);
                    runtime.finish_job(id, true).await;
                    QueueResponse::Acked { job }
                }
                Err(err) => QueueResponse::Error {
                    message: err.to_string(),
                },
            }
        }
        Ok(QueueRequest::Heartbeat { id }) => {
            if runtime.heartbeat(id).await {
                QueueResponse::HeartbeatRecorded { id }
            } else {
                QueueResponse::Error {
                    message: format!("job {id} is not being tracked for heartbeat"),
                }
            }
        }
        Ok(QueueRequest::Nack { id, reason }) => {
            let mut guard = queue.lock().await;
            match guard.nack(id, reason) {
                Ok(job) => {
                    drop(guard);
                    runtime.finish_job(id, false).await;
                    QueueResponse::Nacked { job }
                }
                Err(err) => QueueResponse::Error {
                    message: err.to_string(),
                },
            }
        }
        Ok(QueueRequest::Subscribe { queue: queue_name }) => {
            let sender = subscription_sender(subscriptions, &queue_name).await;
            subscriptions_rx.push(sender.subscribe());
            QueueResponse::Subscribed { queue: queue_name }
        }
        Ok(QueueRequest::Stats) => {
            let guard = queue.lock().await;
            QueueResponse::Stats {
                pending: guard.queue().pending_len(),
                processing: guard.queue().processing_len(),
                scheduled: guard.queue().scheduled_len(),
                completed: guard.queue().completed_len(),
                dead_letter: guard.queue().dead_letter_len(),
            }
        }
        Err(err) => QueueResponse::Error {
            message: format!("invalid request: {}", err),
        },
    }
}

async fn next_subscription_event(
    subscriptions_rx: &mut Vec<tokio::sync::broadcast::Receiver<QueueResponse>>,
) -> Option<QueueResponse> {
    let mut index = 0usize;
    while index < subscriptions_rx.len() {
        match subscriptions_rx[index].try_recv() {
            Ok(event) => return Some(event),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                index += 1;
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Closed) => {
                subscriptions_rx.remove(index);
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {
                index += 1;
            }
        }
    }
    None
}

async fn run_scheduled_job_promoter(
    queue: SharedPersistentQueue,
    subscriptions: SubscriptionMap,
) {
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
    loop {
        interval.tick().await;

        let promoted_jobs = {
            let mut guard = queue.lock().await;
            let before = guard.queue().pending_len();
            let _promoted = guard.queue_mut().promote_ready_jobs(Utc::now());
            let after = guard.queue().pending_len();
            if after <= before {
                Vec::new()
            } else {
                guard
                    .queue()
                    .pending
                    .iter()
                    .rev()
                    .take(after - before)
                    .cloned()
                    .collect::<Vec<_>>()
            }
        };

        for job in promoted_jobs {
            broadcast_job_event(&subscriptions, job.queue.clone(), "scheduled_ready", job).await;
        }
    }
}

async fn run_cron_scheduler(
    queue: SharedPersistentQueue,
    subscriptions: SubscriptionMap,
    recurring_jobs: RecurringJobStore,
) {
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
    loop {
        interval.tick().await;
        let now = Utc::now();
        let mut due_jobs = Vec::new();

        {
            let mut guard = recurring_jobs.lock().await;
            for recurring in guard.iter_mut() {
                while recurring.next_run_at <= now {
                    due_jobs.push(Job::new(
                        recurring.queue.clone(),
                        serde_json::to_vec(&recurring.payload).unwrap_or_default(),
                        recurring.max_attempts,
                    ));
                    recurring.next_run_at = recurring
                        .schedule
                        .after(&recurring.next_run_at)
                        .next()
                        .or_else(|| recurring.schedule.upcoming(Utc).next())
                        .unwrap_or_else(Utc::now);
                }
            }
        }

        for job in due_jobs {
            let queue_name = job.queue.clone();
            let mut guard = queue.lock().await;
            if guard.enqueue(job.clone()).is_ok() {
                drop(guard);
                broadcast_job_event(&subscriptions, queue_name, "cron_enqueued", job).await;
            }
        }
    }
}

async fn run_heartbeat_monitor(queue: SharedPersistentQueue, runtime: QueueRuntimeState) {
    let poll_every = runtime.config.heartbeat_interval.min(StdDuration::from_secs(1));
    let mut interval = tokio::time::interval(poll_every);
    loop {
        interval.tick().await;

        let stale_job_ids = {
            let active = runtime.active_jobs.lock().await;
            active
                .iter()
                .filter_map(|(job_id, active)| {
                    (active.last_heartbeat.elapsed() > runtime.config.heartbeat_timeout)
                        .then_some(*job_id)
                })
                .collect::<Vec<_>>()
        };

        for job_id in stale_job_ids {
            let mut guard = queue.lock().await;
            let _ = guard.nack(job_id, "heartbeat timeout");
            drop(guard);
            runtime.finish_job(job_id, false).await;
        }
    }
}

async fn get_queue_stats(
    State((queue, runtime)): State<(SharedPersistentQueue, QueueRuntimeState)>,
    AxumPath(queue_name): AxumPath<String>,
) -> Json<QueueStatsView> {
    let guard = queue.lock().await;
    Json(runtime.stats_for_queue(guard.queue(), &queue_name).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use futures_util::{SinkExt, StreamExt};
    use tempfile::TempDir;
    use tower::ServiceExt;
    use tokio::net::TcpListener;
    use tokio_tungstenite::{connect_async, tungstenite::Message};

    fn make_job(queue: &str, payload: &[u8], max_attempts: u32) -> Job {
        Job::new(queue.to_string(), payload.to_vec(), max_attempts)
    }

    fn temp_queue_dir() -> TempDir {
        tempfile::tempdir().expect("temporary directory should be created")
    }

    async fn available_port() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("port probe should bind");
        let port = listener
            .local_addr()
            .expect("local addr should exist")
            .port();
        drop(listener);
        port
    }

    #[test]
    fn core_queue_starts_empty() {
        let queue = Queue::new();

        assert_eq!(queue.pending_len(), 0);
        assert_eq!(queue.processing_len(), 0);
        assert_eq!(queue.completed_len(), 0);
        assert_eq!(queue.dead_letter_len(), 0);
        assert!(queue.is_empty());
    }

    #[test]
    fn scheduled_job_keeps_timestamp() {
        let when = Utc::now() + Duration::minutes(5);
        let job = Job::scheduled("reports", b"{}".to_vec(), 4, when);

        assert_eq!(job.scheduled_at, Some(when));
        assert_eq!(job.status, JobStatus::Pending);
    }

    #[test]
    fn dequeue_moves_job_to_processing() {
        let mut queue = Queue::new();
        let job = make_job("jobs", b"1", 3);
        let id = job.id;
        queue.enqueue(job);

        let dequeued = queue.dequeue().expect("job should dequeue");

        assert_eq!(dequeued.id, id);
        assert_eq!(dequeued.status, JobStatus::Processing);
        assert_eq!(queue.pending_len(), 0);
        assert_eq!(queue.processing_len(), 1);
    }

    #[test]
    fn ack_marks_job_completed() {
        let mut queue = Queue::new();
        let job = make_job("jobs", b"1", 3);
        let id = job.id;
        queue.enqueue(job);
        let _ = queue.dequeue();

        let acked = queue.ack(id).expect("ack should succeed");

        assert_eq!(acked.status, JobStatus::Completed);
        assert_eq!(queue.completed_len(), 1);
        assert_eq!(queue.processing_len(), 0);
    }

    #[test]
    fn nack_requeues_before_dead_letter() {
        let mut queue = Queue::new();
        let job = make_job("jobs", b"1", 2);
        let id = job.id;
        queue.enqueue(job);
        let _ = queue.dequeue();

        let nacked = queue.nack(id, "retry").expect("nack should succeed");

        assert_eq!(nacked.status, JobStatus::Pending);
        assert_eq!(nacked.attempts, 1);
        assert_eq!(queue.pending_len(), 1);
        assert_eq!(queue.dead_letter_len(), 0);
    }

    #[test]
    fn nack_dead_letters_when_attempts_exhausted() {
        let mut queue = Queue::new();
        let job = make_job("jobs", b"1", 1);
        let id = job.id;
        queue.enqueue(job);
        let _ = queue.dequeue();

        let nacked = queue.nack(id, "fatal").expect("nack should succeed");

        assert_eq!(nacked.status, JobStatus::DeadLetter);
        assert_eq!(queue.dead_letter_len(), 1);
        assert_eq!(queue.failure_reason(id), Some("fatal"));
    }

    #[test]
    fn wal_event_round_trips_through_bincode() {
        let event = WalEvent::Enqueue(make_job("jobs", b"payload", 3));

        let bytes = bincode::serialize(&event).expect("event should serialize");
        let decoded: WalEvent = bincode::deserialize(&bytes).expect("event should deserialize");

        assert_eq!(decoded, event);
    }

    #[test]
    fn wal_uses_length_prefixed_records() {
        let dir = temp_queue_dir();
        let job = make_job("jobs", b"abc", 2);
        let event = WalEvent::Enqueue(job.clone());
        let expected = bincode::serialize(&event).expect("event should serialize");

        {
            let mut queue = PersistentQueue::open(dir.path()).expect("queue should open");
            queue.enqueue(job).expect("enqueue should persist");
        }

        let wal_path = dir.path().join(DEFAULT_WAL_FILE);
        let mut file = File::open(wal_path).expect("wal file should exist");
        let mut len_buf = [0u8; 4];
        file.read_exact(&mut len_buf).expect("length prefix should exist");
        let length = u32::from_le_bytes(len_buf) as usize;
        let mut payload = vec![0u8; length];
        file.read_exact(&mut payload).expect("payload should exist");

        assert_eq!(length, expected.len());
        assert_eq!(payload, expected);
    }

    #[test]
    fn persistent_queue_recovers_enqueued_jobs_on_restart() {
        let dir = temp_queue_dir();
        let job = make_job("emails", b"welcome", 3);
        let job_id = job.id;

        {
            let mut queue = PersistentQueue::open(dir.path()).expect("queue should open");
            queue.enqueue(job).expect("enqueue should persist");
        }

        let reopened = PersistentQueue::open(dir.path()).expect("queue should reopen");

        assert_eq!(reopened.queue().pending_len(), 1);
        assert_eq!(reopened.queue().pending.front().map(|job| job.id), Some(job_id));
    }

    #[test]
    fn persistent_queue_recovers_processing_state_after_dequeue() {
        let dir = temp_queue_dir();
        let job = make_job("jobs", b"1", 3);
        let job_id = job.id;

        {
            let mut queue = PersistentQueue::open(dir.path()).expect("queue should open");
            queue.enqueue(job).expect("enqueue should persist");
            let dequeued = queue.dequeue().expect("dequeue should persist");
            assert_eq!(dequeued.map(|job| job.id), Some(job_id));
        }

        let reopened = PersistentQueue::open(dir.path()).expect("queue should reopen");

        assert_eq!(reopened.queue().pending_len(), 0);
        assert_eq!(reopened.queue().processing_len(), 1);
        assert!(reopened.queue().processing.contains_key(&job_id));
    }

    #[test]
    fn persistent_queue_recovers_ack_state_after_restart() {
        let dir = temp_queue_dir();
        let job = make_job("jobs", b"1", 3);
        let job_id = job.id;

        {
            let mut queue = PersistentQueue::open(dir.path()).expect("queue should open");
            queue.enqueue(job).expect("enqueue should persist");
            let _ = queue.dequeue().expect("dequeue should persist");
            let _ = queue.ack(job_id).expect("ack should persist");
        }

        let reopened = PersistentQueue::open(dir.path()).expect("queue should reopen");

        assert_eq!(reopened.queue().completed_len(), 1);
        assert_eq!(reopened.queue().completed_jobs()[0].id, job_id);
    }

    #[test]
    fn persistent_queue_recovers_dead_letter_state_after_restart() {
        let dir = temp_queue_dir();
        let job = make_job("jobs", b"1", 1);
        let job_id = job.id;

        {
            let mut queue = PersistentQueue::open(dir.path()).expect("queue should open");
            queue.enqueue(job).expect("enqueue should persist");
            let _ = queue.dequeue().expect("dequeue should persist");
            let _ = queue.nack(job_id, "fatal").expect("nack should persist");
        }

        let reopened = PersistentQueue::open(dir.path()).expect("queue should reopen");

        assert_eq!(reopened.queue().dead_letter_len(), 1);
        assert_eq!(reopened.queue().dead_letter_jobs()[0].id, job_id);
        assert_eq!(reopened.queue().failure_reason(job_id), Some("fatal"));
    }

    #[test]
    fn rotation_creates_snapshot_and_cleans_old_segments() {
        let dir = temp_queue_dir();
        let mut queue =
            PersistentQueue::open_with_max_bytes(dir.path(), 1).expect("queue should open");

        queue
            .enqueue(make_job("jobs", b"payload", 3))
            .expect("enqueue should persist");

        assert!(dir.path().join(SNAPSHOT_FILE).exists());
        assert!(dir.path().join(DEFAULT_WAL_FILE).exists());
        assert!(!dir.path().join("wal.001.log").exists());
    }

    #[test]
    fn manual_compaction_rebuilds_from_snapshot_after_reopen() {
        let dir = temp_queue_dir();
        let job = make_job("jobs", b"payload", 3);
        let job_id = job.id;

        {
            let mut queue = PersistentQueue::open(dir.path()).expect("queue should open");
            queue.enqueue(job).expect("enqueue should persist");
            queue.compact().expect("manual compaction should succeed");
        }

        let reopened = PersistentQueue::open(dir.path()).expect("queue should reopen");

        assert_eq!(reopened.queue().pending_len(), 1);
        assert_eq!(reopened.queue().pending.front().map(|job| job.id), Some(job_id));
    }

    #[test]
    fn replay_ignores_truncated_tail_record_after_simulated_crash() {
        let dir = temp_queue_dir();
        let wal = Wal::open(dir.path(), DEFAULT_MAX_WAL_BYTES).expect("wal should open");
        let first_job = make_job("jobs", b"first", 3);
        let second_job = make_job("jobs", b"second", 3);
        let first_event = WalEvent::Enqueue(first_job.clone());
        let second_event = WalEvent::Enqueue(second_job);
        let first_bytes = bincode::serialize(&first_event).expect("first event should serialize");
        let second_bytes =
            bincode::serialize(&second_event).expect("second event should serialize");

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.path().join(DEFAULT_WAL_FILE))
            .expect("wal file should open");
        file.write_all(&(first_bytes.len() as u32).to_le_bytes())
            .expect("first length should write");
        file.write_all(&first_bytes)
            .expect("first payload should write");
        file.write_all(&(second_bytes.len() as u32).to_le_bytes())
            .expect("second length should write");
        file.write_all(&second_bytes[..second_bytes.len() / 2])
            .expect("partial payload should write");
        file.sync_all().expect("wal file should sync");

        let recovered = wal.load_queue().expect("queue should recover");

        assert_eq!(recovered.pending_len(), 1);
        assert_eq!(recovered.pending.front().map(|job| job.id), Some(first_job.id));
    }

    #[test]
    fn interrupted_enqueue_keeps_all_committed_jobs_after_restart() {
        let dir = temp_queue_dir();
        let committed_job = make_job("jobs", b"committed", 3);
        let interrupted_job = make_job("jobs", b"interrupted", 3);
        let committed_id = committed_job.id;

        {
            let mut queue = PersistentQueue::open(dir.path()).expect("queue should open");
            queue
                .enqueue(committed_job)
                .expect("committed enqueue should persist");
        }

        let interrupted_event = WalEvent::Enqueue(interrupted_job);
        let interrupted_bytes =
            bincode::serialize(&interrupted_event).expect("interrupted event should serialize");

        let mut file = OpenOptions::new()
            .append(true)
            .open(dir.path().join(DEFAULT_WAL_FILE))
            .expect("wal file should open");
        file.write_all(&(interrupted_bytes.len() as u32).to_le_bytes())
            .expect("interrupted length should write");
        file.write_all(&interrupted_bytes[..interrupted_bytes.len() / 3])
            .expect("interrupted payload should partially write");
        file.sync_all().expect("wal file should sync");

        let recovered = PersistentQueue::open(dir.path()).expect("queue should recover");

        assert_eq!(recovered.queue().pending_len(), 1);
        assert_eq!(
            recovered.queue().pending.front().map(|job| job.id),
            Some(committed_id)
        );
    }

    #[test]
    fn promote_ready_jobs_moves_due_delayed_job_into_pending() {
        let mut queue = Queue::new();
        let run_at = Utc::now() + Duration::milliseconds(50);
        let job = Job::scheduled("emails", br#"{"kind":"welcome"}"#.to_vec(), 3, run_at);
        let job_id = job.id;
        queue.enqueue(job);

        assert_eq!(queue.pending_len(), 0);
        assert_eq!(queue.scheduled_len(), 1);

        let promoted = queue.promote_ready_jobs(run_at + Duration::milliseconds(1));

        assert_eq!(promoted, 1);
        assert_eq!(queue.pending_len(), 1);
        assert_eq!(queue.scheduled_len(), 0);
        assert_eq!(queue.pending.front().map(|job| job.id), Some(job_id));
    }

    #[tokio::test]
    async fn websocket_api_supports_enqueue_and_stats() {
        let dir = temp_queue_dir();
        let port = available_port().await;
        let server = QueueWebSocketServer::new(
            PersistentQueue::open(dir.path()).expect("queue should open"),
            QueueWsConfig {
                host: "127.0.0.1".to_string(),
                port,
            },
        );

        let server_task = tokio::spawn(async move {
            let _ = server.bind().await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        let (mut socket, _) = connect_async(format!("ws://127.0.0.1:{port}"))
            .await
            .expect("client should connect");

        let enqueue = serde_json::to_string(&QueueRequest::Enqueue {
            queue: "emails".to_string(),
            payload: serde_json::json!({ "to": "user@example.com" }),
            max_attempts: 3,
        })
        .expect("request should serialize");
        socket
            .send(Message::Text(enqueue))
            .await
            .expect("enqueue should send");

        let response = socket.next().await.expect("response expected").expect("response ok");
        let response: QueueResponse = serde_json::from_str(
            response.to_text().expect("text response expected"),
        )
        .expect("response should deserialize");
        match response {
            QueueResponse::Enqueued { job } => assert_eq!(job.queue, "emails"),
            other => panic!("unexpected response: {:?}", other),
        }

        let stats = serde_json::to_string(&QueueRequest::Stats).expect("request should serialize");
        socket
            .send(Message::Text(stats))
            .await
            .expect("stats should send");
        let response = socket.next().await.expect("response expected").expect("response ok");
        let response: QueueResponse = serde_json::from_str(
            response.to_text().expect("text response expected"),
        )
        .expect("response should deserialize");
        match response {
            QueueResponse::Stats { pending, processing, .. } => {
                assert_eq!(pending, 1);
                assert_eq!(processing, 0);
            }
            other => panic!("unexpected response: {:?}", other),
        }

        server_task.abort();
    }

    #[tokio::test]
    async fn websocket_api_supports_scheduled_jobs_and_stats() {
        let dir = temp_queue_dir();
        let port = available_port().await;
        let server = QueueWebSocketServer::new(
            PersistentQueue::open(dir.path()).expect("queue should open"),
            QueueWsConfig {
                host: "127.0.0.1".to_string(),
                port,
            },
        );

        let server_task = tokio::spawn(async move {
            let _ = server.bind().await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        let (mut socket, _) = connect_async(format!("ws://127.0.0.1:{port}"))
            .await
            .expect("client should connect");

        let run_at = Utc::now() + Duration::milliseconds(300);
        let schedule = serde_json::json!({
            "op": "schedule",
            "queue": "emails",
            "payload": { "to": "scheduled@example.com" },
            "run_at": run_at.to_rfc3339(),
            "max_attempts": 3
        });
        socket
            .send(Message::Text(schedule.to_string()))
            .await
            .expect("schedule should send");

        let response = socket.next().await.expect("response expected").expect("response ok");
        let response: QueueResponse = serde_json::from_str(
            response.to_text().expect("text response expected"),
        )
        .expect("response should deserialize");
        match response {
            QueueResponse::Enqueued { job } => {
                assert_eq!(job.queue, "emails");
                assert_eq!(job.scheduled_at.expect("scheduled_at"), run_at);
            }
            other => panic!("unexpected response: {:?}", other),
        }

        socket
            .send(Message::Text(serde_json::to_string(&QueueRequest::Stats).expect("serialize")))
            .await
            .expect("stats should send");
        let response = socket.next().await.expect("response expected").expect("response ok");
        let response: QueueResponse = serde_json::from_str(
            response.to_text().expect("text response expected"),
        )
        .expect("response should deserialize");
        match response {
            QueueResponse::Stats { pending, scheduled, .. } => {
                assert_eq!(pending, 0);
                assert_eq!(scheduled, 1);
            }
            other => panic!("unexpected response: {:?}", other),
        }

        let started = std::time::Instant::now();
        loop {
            socket
                .send(Message::Text(
                    serde_json::to_string(&QueueRequest::Dequeue {
                        queue: Some("emails".to_string()),
                    })
                    .expect("serialize"),
                ))
                .await
                .expect("dequeue should send");
            let response = socket.next().await.expect("response expected").expect("response ok");
            let response: QueueResponse = serde_json::from_str(
                response.to_text().expect("text response expected"),
            )
            .expect("response should deserialize");
            if let QueueResponse::Dequeued { job: Some(job) } = response {
                assert_eq!(job.queue, "emails");
                let elapsed = started.elapsed();
                assert!(elapsed >= std::time::Duration::from_millis(200));
                assert!(elapsed <= std::time::Duration::from_millis(700));
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        server_task.abort();
    }

    #[tokio::test]
    async fn websocket_api_supports_cron_registration_and_emission() {
        let dir = temp_queue_dir();
        let port = available_port().await;
        let server = QueueWebSocketServer::new(
            PersistentQueue::open(dir.path()).expect("queue should open"),
            QueueWsConfig {
                host: "127.0.0.1".to_string(),
                port,
            },
        );

        let server_task = tokio::spawn(async move {
            let _ = server.bind().await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        let (mut socket, _) = connect_async(format!("ws://127.0.0.1:{port}"))
            .await
            .expect("client should connect");

        let cron_request = serde_json::json!({
            "op": "cron",
            "queue": "cron-jobs",
            "expression": "*/1 * * * * * *",
            "payload": { "kind": "heartbeat" },
            "max_attempts": 2
        });
        socket
            .send(Message::Text(cron_request.to_string()))
            .await
            .expect("cron should send");

        let response = socket.next().await.expect("response expected").expect("response ok");
        let response: QueueResponse = serde_json::from_str(
            response.to_text().expect("text response expected"),
        )
        .expect("response should deserialize");
        assert!(matches!(response, QueueResponse::CronRegistered { .. }));

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut found = false;
        while tokio::time::Instant::now() < deadline {
            socket
                .send(Message::Text(
                    serde_json::to_string(&QueueRequest::Dequeue {
                        queue: Some("cron-jobs".to_string()),
                    })
                    .expect("serialize"),
                ))
                .await
                .expect("dequeue should send");

            let response = socket.next().await.expect("response expected").expect("response ok");
            let response: QueueResponse = serde_json::from_str(
                response.to_text().expect("text response expected"),
            )
            .expect("response should deserialize");
            if matches!(response, QueueResponse::Dequeued { job: Some(_) }) {
                found = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        assert!(found, "expected cron job to be enqueued");
        server_task.abort();
    }

    #[tokio::test]
    async fn websocket_api_enforces_per_queue_concurrency_limit() {
        let dir = temp_queue_dir();
        let port = available_port().await;
        let server = QueueWebSocketServer::new(
            PersistentQueue::open(dir.path()).expect("queue should open"),
            QueueWsConfig {
                host: "127.0.0.1".to_string(),
                port,
            },
        );
        server.set_queue_concurrency("emails", 1).await;

        let server_task = tokio::spawn(async move {
            let _ = server.bind().await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        let (mut socket, _) = connect_async(format!("ws://127.0.0.1:{port}"))
            .await
            .expect("client should connect");

        for subject in ["one", "two"] {
            socket
                .send(Message::Text(
                    serde_json::json!({
                        "op": "enqueue",
                        "queue": "emails",
                        "payload": { "subject": subject },
                        "max_attempts": 3
                    })
                    .to_string(),
                ))
                .await
                .expect("enqueue should send");
            let _ = socket.next().await.expect("response expected").expect("response ok");
        }

        socket
            .send(Message::Text(
                serde_json::json!({
                    "op": "dequeue",
                    "queue": "emails"
                })
                .to_string(),
            ))
            .await
            .expect("first dequeue should send");
        let first = socket.next().await.expect("response expected").expect("response ok");
        let first: QueueResponse =
            serde_json::from_str(first.to_text().expect("text expected")).expect("deserialize");
        assert!(matches!(first, QueueResponse::Dequeued { job: Some(_) }));

        socket
            .send(Message::Text(
                serde_json::json!({
                    "op": "dequeue",
                    "queue": "emails"
                })
                .to_string(),
            ))
            .await
            .expect("second dequeue should send");
        let second = socket.next().await.expect("response expected").expect("response ok");
        let second: QueueResponse =
            serde_json::from_str(second.to_text().expect("text expected")).expect("deserialize");
        assert!(matches!(second, QueueResponse::Dequeued { job: None }));

        server_task.abort();
    }

    #[tokio::test]
    async fn heartbeat_timeout_auto_nacks_processing_job() {
        let dir = temp_queue_dir();
        let port = available_port().await;
        let server = QueueWebSocketServer::new(
            PersistentQueue::open(dir.path()).expect("queue should open"),
            QueueWsConfig {
                host: "127.0.0.1".to_string(),
                port,
            },
        )
        .with_worker_pool_config(WorkerPoolConfig {
            default_concurrency: 1,
            heartbeat_interval: StdDuration::from_millis(20),
            heartbeat_timeout: StdDuration::from_millis(60),
        });
        server.set_queue_concurrency("emails", 1).await;

        let server_task = tokio::spawn(async move {
            let _ = server.bind().await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        let (mut socket, _) = connect_async(format!("ws://127.0.0.1:{port}"))
            .await
            .expect("client should connect");

        socket
            .send(Message::Text(
                serde_json::json!({
                    "op": "enqueue",
                    "queue": "emails",
                    "payload": { "subject": "timeout" },
                    "max_attempts": 2
                })
                .to_string(),
            ))
            .await
            .expect("enqueue should send");
        let _ = socket.next().await.expect("response expected").expect("response ok");

        socket
            .send(Message::Text(
                serde_json::json!({
                    "op": "dequeue",
                    "queue": "emails"
                })
                .to_string(),
            ))
            .await
            .expect("dequeue should send");
        let response = socket.next().await.expect("response expected").expect("response ok");
        let response: QueueResponse =
            serde_json::from_str(response.to_text().expect("text expected")).expect("deserialize");
        let job = match response {
            QueueResponse::Dequeued { job: Some(job) } => job,
            other => panic!("unexpected response: {:?}", other),
        };

        tokio::time::sleep(std::time::Duration::from_millis(160)).await;

        socket
            .send(Message::Text(
                serde_json::json!({
                    "op": "stats"
                })
                .to_string(),
            ))
            .await
            .expect("stats should send");
        let _ = socket.next().await.expect("response expected").expect("response ok");

        let reopened = PersistentQueue::open(dir.path()).expect("queue should recover");
        assert_eq!(reopened.queue().failure_reason(job.id), Some("heartbeat timeout"));
        assert_eq!(reopened.queue().pending_len(), 1);

        server_task.abort();
    }

    #[tokio::test]
    async fn websocket_heartbeat_keeps_job_alive() {
        let dir = temp_queue_dir();
        let port = available_port().await;
        let server = QueueWebSocketServer::new(
            PersistentQueue::open(dir.path()).expect("queue should open"),
            QueueWsConfig {
                host: "127.0.0.1".to_string(),
                port,
            },
        )
        .with_worker_pool_config(WorkerPoolConfig {
            default_concurrency: 1,
            heartbeat_interval: StdDuration::from_millis(20),
            heartbeat_timeout: StdDuration::from_millis(80),
        });
        server.set_queue_concurrency("emails", 1).await;

        let server_task = tokio::spawn(async move {
            let _ = server.bind().await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        let (mut socket, _) = connect_async(format!("ws://127.0.0.1:{port}"))
            .await
            .expect("client should connect");

        socket
            .send(Message::Text(
                serde_json::json!({
                    "op": "enqueue",
                    "queue": "emails",
                    "payload": { "subject": "alive" },
                    "max_attempts": 2
                })
                .to_string(),
            ))
            .await
            .expect("enqueue should send");
        let _ = socket.next().await.expect("response expected").expect("response ok");

        socket
            .send(Message::Text(
                serde_json::json!({
                    "op": "dequeue",
                    "queue": "emails"
                })
                .to_string(),
            ))
            .await
            .expect("dequeue should send");
        let response = socket.next().await.expect("response expected").expect("response ok");
        let response: QueueResponse =
            serde_json::from_str(response.to_text().expect("text expected")).expect("deserialize");
        let job = match response {
            QueueResponse::Dequeued { job: Some(job) } => job,
            other => panic!("unexpected response: {:?}", other),
        };

        for _ in 0..3 {
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
            socket
                .send(Message::Text(
                    serde_json::json!({
                        "op": "heartbeat",
                        "id": job.id
                    })
                    .to_string(),
                ))
                .await
                .expect("heartbeat should send");
            let response = socket.next().await.expect("response expected").expect("response ok");
            let response: QueueResponse = serde_json::from_str(
                response.to_text().expect("text expected"),
            )
            .expect("deserialize");
            assert!(matches!(response, QueueResponse::HeartbeatRecorded { .. }));
        }

        let reopened = PersistentQueue::open(dir.path()).expect("queue should reopen");
        assert_eq!(reopened.queue().failure_reason(job.id), None);
        assert_eq!(reopened.queue().processing_len(), 1);

        server_task.abort();
    }

    #[tokio::test]
    async fn stats_router_returns_queue_specific_stats_and_throughput() {
        let dir = temp_queue_dir();
        let queue = PersistentQueue::open(dir.path()).expect("queue should open");
        let server = QueueWebSocketServer::new(queue, QueueWsConfig::default());
        server.set_queue_concurrency("emails", 2).await;

        {
            let mut guard = server.queue.lock().await;
            guard
                .enqueue(make_job("emails", b"one", 3))
                .expect("enqueue should persist");
            guard
                .enqueue(make_job("emails", b"two", 3))
                .expect("enqueue should persist");
            let job = guard
                .dequeue_for_queue("emails")
                .expect("dequeue should work")
                .expect("job expected");
            server
                .runtime
                .register_processing("emails", job.id, server.runtime.acquire_permit("emails").await.expect("permit").expect("available"))
                .await;
            let _ = guard.ack(job.id).expect("ack should work");
            server.runtime.finish_job(job.id, true).await;
        }

        let response = server
            .stats_router()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/queues/emails/stats")
                    .body(axum::body::Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read");
        let stats: QueueStatsView =
            serde_json::from_slice(&body).expect("response should deserialize");

        assert_eq!(stats.queue, "emails");
        assert_eq!(stats.pending, 1);
        assert_eq!(stats.processing, 0);
        assert_eq!(stats.failed, 0);
        assert!(stats.throughput_per_sec >= 1.0);
    }

    #[tokio::test]
    async fn websocket_api_supports_subscribe_and_dequeue_ack_flow() {
        let dir = temp_queue_dir();
        let port = available_port().await;
        let server = QueueWebSocketServer::new(
            PersistentQueue::open(dir.path()).expect("queue should open"),
            QueueWsConfig {
                host: "127.0.0.1".to_string(),
                port,
            },
        );

        let server_task = tokio::spawn(async move {
            let _ = server.bind().await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        let (mut subscriber, _) = connect_async(format!("ws://127.0.0.1:{port}"))
            .await
            .expect("subscriber should connect");
        let subscribe = serde_json::to_string(&QueueRequest::Subscribe {
            queue: "emails".to_string(),
        })
        .expect("request should serialize");
        subscriber
            .send(Message::Text(subscribe))
            .await
            .expect("subscribe should send");
        let subscribed = subscriber.next().await.expect("ack expected").expect("ack ok");
        let subscribed: QueueResponse = serde_json::from_str(
            subscribed.to_text().expect("text response expected"),
        )
        .expect("response should deserialize");
        assert!(matches!(subscribed, QueueResponse::Subscribed { .. }));

        let (mut client, _) = connect_async(format!("ws://127.0.0.1:{port}"))
            .await
            .expect("client should connect");
        let enqueue = serde_json::to_string(&QueueRequest::Enqueue {
            queue: "emails".to_string(),
            payload: serde_json::json!({ "subject": "hello" }),
            max_attempts: 3,
        })
        .expect("request should serialize");
        client
            .send(Message::Text(enqueue))
            .await
            .expect("enqueue should send");
        let _ = client.next().await.expect("enqueue response").expect("enqueue ok");

        let event = subscriber
            .next()
            .await
            .expect("subscription event expected")
            .expect("subscription event ok");
        let event: QueueResponse = serde_json::from_str(event.to_text().expect("text expected"))
            .expect("event should deserialize");
        let enqueued_job = match event {
            QueueResponse::Event { queue, kind, job } => {
                assert_eq!(queue, "emails");
                assert_eq!(kind, "enqueued");
                job
            }
            other => panic!("unexpected event: {:?}", other),
        };

        let dequeue = serde_json::to_string(&QueueRequest::Dequeue {
            queue: Some("emails".to_string()),
        })
        .expect("request should serialize");
        client
            .send(Message::Text(dequeue))
            .await
            .expect("dequeue should send");
        let response = client.next().await.expect("dequeue response").expect("dequeue ok");
        let response: QueueResponse = serde_json::from_str(response.to_text().expect("text expected"))
            .expect("response should deserialize");
        let dequeued_job = match response {
            QueueResponse::Dequeued { job: Some(job) } => job,
            other => panic!("unexpected dequeue response: {:?}", other),
        };
        assert_eq!(dequeued_job.id, enqueued_job.id);

        let ack = serde_json::to_string(&QueueRequest::Ack {
            id: dequeued_job.id,
        })
        .expect("request should serialize");
        client
            .send(Message::Text(ack))
            .await
            .expect("ack should send");
        let response = client.next().await.expect("ack response").expect("ack ok");
        let response: QueueResponse = serde_json::from_str(response.to_text().expect("text expected"))
            .expect("response should deserialize");
        match response {
            QueueResponse::Acked { job } => assert_eq!(job.id, dequeued_job.id),
            other => panic!("unexpected ack response: {:?}", other),
        }

        server_task.abort();
    }
}
