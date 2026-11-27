use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use tokio::sync::oneshot;

use super::*;
use crate::builtin::Tool;

/// A job the test finishes by hand.
struct Controlled {
    id: JobId,
    handle: JobHandle,
    finish: Option<oneshot::Sender<JobOutcome>>,
    cancelled: Arc<Mutex<Vec<Option<String>>>>,
}

impl Controlled {
    fn end(&mut self, outcome: JobOutcome) {
        let _ = self.finish.take().expect("not ended yet").send(outcome);
    }
}

/// The handle the starter was given, and the reasons its cancel was called with.
type Given = Arc<Mutex<Option<JobHandle>>>;
type Kills = Arc<Mutex<Vec<Option<String>>>>;

fn spec(kind: &str, owner: Option<&str>) -> (JobSpec, Given, oneshot::Sender<JobOutcome>, Kills) {
    let (finish, done) = oneshot::channel();
    let handle = Given::default();
    let cancelled = Kills::default();
    let (seen, kills) = (Arc::clone(&handle), Arc::clone(&cancelled));
    let spec = JobSpec {
        kind: kind.to_owned(),
        label: "a job".to_owned(),
        owner: owner.map(str::to_owned),
        output_limit_bytes: None,
        output: Vec::new(),
        run: Box::new(move |given| {
            *seen.lock().unwrap() = Some(given);
            JobHooks {
                done: Box::pin(async move {
                    done.await.unwrap_or_else(|_| JobOutcome::failed("dropped"))
                }),
                cancel: Arc::new(move |why| kills.lock().unwrap().push(why.map(str::to_owned))),
            }
        }),
    };
    (spec, handle, finish, cancelled)
}

fn start(registry: &Registry, owner: Option<&str>) -> Controlled {
    let (spec, handle, finish, cancelled) = spec("bash", owner);
    let id = registry.start(spec).expect("admitted");
    let handle = handle.lock().unwrap().take().expect("started");
    Controlled {
        id,
        handle,
        finish: Some(finish),
        cancelled,
    }
}

async fn settled(registry: &Registry, id: &str, caller: Option<&str>) -> JobView {
    registry
        .wait(id, Duration::from_secs(5), caller)
        .await
        .expect("known")
}

fn ended(status: JobStatus, detail: &str) -> JobOutcome {
    JobOutcome {
        status,
        detail: Some(detail.to_owned()),
        result: None,
    }
}

#[tokio::test]
async fn ids_carry_the_kind_and_the_limit_counts_only_live_jobs_of_one_owner() {
    let registry = Registry::new(Limits {
        max_active_per_owner: 1,
        ..Limits::default()
    });
    let mut first = start(&registry, Some("a"));
    assert!(first.id.starts_with("bash-") && first.id.len() == "bash-".len() + 16);
    let (refused, ..) = spec("bash", Some("a"));
    assert_eq!(registry.start(refused), Err(JobError::Limit(1)));
    let _other = start(&registry, Some("b"));
    first.end(JobOutcome::completed());
    settled(&registry, &first.id, Some("a")).await;
    let _next = start(&registry, Some("a"));
}

#[tokio::test]
async fn an_empty_kind_or_label_or_a_zero_limit_is_refused() {
    let registry = Registry::default();
    let (blank, ..) = spec("", None);
    assert!(matches!(registry.start(blank), Err(JobError::Invalid(_))));
    let (mut unlabelled, ..) = spec("bash", None);
    unlabelled.label.clear();
    assert!(matches!(
        registry.start(unlabelled),
        Err(JobError::Invalid(_))
    ));
    let (mut capped, ..) = spec("bash", None);
    capped.output_limit_bytes = Some(0);
    assert!(matches!(registry.start(capped), Err(JobError::Invalid(_))));
}

#[tokio::test]
async fn another_owner_cannot_see_or_touch_a_job() {
    let registry = Registry::default();
    let job = start(&registry, Some("a"));
    let foreign = JobError::Foreign(job.id.clone());
    assert_eq!(registry.get(&job.id, Some("b")).unwrap_err(), foreign);
    assert_eq!(registry.kill(&job.id, None, None).unwrap_err(), foreign);
    assert!(registry.list(Some("b")).is_empty());
    assert_eq!(registry.list(Some("a")).len(), 1);
    assert_eq!(
        registry.get("bash-nope", None).unwrap_err(),
        JobError::Unknown("bash-nope".to_owned())
    );
}

#[tokio::test]
async fn reading_moves_on_read_at_does_not_and_a_result_comes_once() {
    let registry = Registry::default();
    let mut job = start(&registry, None);
    job.handle.append("one ", &JobAppendOptions::default());
    assert_eq!(registry.read(&job.id, None).unwrap().chunks[0].text, "one ");
    job.handle.append("two", &JobAppendOptions::default());
    assert_eq!(registry.read_at(&job.id, 0, None).unwrap().chunks.len(), 2);
    assert_eq!(registry.read(&job.id, None).unwrap().chunks[0].text, "two");
    job.end(JobOutcome {
        status: JobStatus::Completed,
        detail: None,
        result: Some("value".to_owned()),
    });
    settled(&registry, &job.id, None).await;
    assert_eq!(
        registry.read(&job.id, None).unwrap().result.as_deref(),
        Some("value")
    );
    assert_eq!(registry.read(&job.id, None).unwrap().result, None);
}

#[tokio::test]
async fn a_kill_asks_once_and_its_reason_joins_the_detail() {
    let registry = Registry::default();
    let mut job = start(&registry, None);
    assert_eq!(
        registry.kill(&job.id, None, Some("not needed")),
        Ok(KillResult::Requested)
    );
    assert_eq!(
        registry.get(&job.id, None).unwrap().status,
        JobStatus::Stopping
    );
    assert_eq!(
        *job.cancelled.lock().unwrap(),
        [Some("not needed".to_owned())]
    );
    assert!(matches!(
        registry.remove(&job.id, None),
        Err(JobError::StillLive(..))
    ));
    job.end(ended(JobStatus::Killed, "signal: 15"));
    let view = settled(&registry, &job.id, None).await;
    assert_eq!(view.detail.as_deref(), Some("signal: 15; not needed"));
    assert_eq!(
        registry.kill(&job.id, None, None),
        Ok(KillResult::AlreadyFinished)
    );
    registry.remove(&job.id, None).expect("terminal");
    assert!(registry.list(None).is_empty());
}

#[tokio::test]
async fn a_job_that_outran_its_kill_keeps_its_own_detail() {
    let registry = Registry::default();
    let mut job = start(&registry, None);
    registry.kill(&job.id, None, Some("too late")).unwrap();
    job.end(ended(JobStatus::Completed, "exit code: 0"));
    assert_eq!(
        settled(&registry, &job.id, None).await.detail.as_deref(),
        Some("exit code: 0")
    );
}

#[tokio::test]
async fn a_wait_that_times_out_leaves_the_job_running() {
    let registry = Registry::default();
    let job = start(&registry, None);
    let view = registry
        .wait(&job.id, Duration::from_millis(20), None)
        .await
        .unwrap();
    assert_eq!(view.status, JobStatus::Running);
    assert!(registry.wait(&job.id, Duration::ZERO, None).await.is_err());
}

#[tokio::test]
async fn events_come_in_order_and_say_whether_someone_was_waiting() {
    let registry = Arc::new(Registry::default());
    let seen: Arc<Mutex<Vec<(&'static str, bool)>>> = Arc::default();
    let into = Arc::clone(&seen);
    let _subscription = registry.subscribe(
        JobEventFilter::All,
        Arc::new(move |event| {
            let awaited = matches!(event, JobEvent::Settled { awaited: true, .. });
            into.lock().unwrap().push((event.type_name(), awaited));
        }),
    );
    let mut job = start(&registry, None);
    let waiting = tokio::spawn({
        let (registry, id) = (Arc::clone(&registry), job.id.clone());
        async move { settled(&registry, &id, None).await }
    });
    tokio::task::yield_now().await;
    job.handle.update_progress("halfway");
    job.end(JobOutcome::completed());
    waiting.await.unwrap();
    // Appends and progress after the end are dropped.
    job.handle.append("late", &JobAppendOptions::default());
    job.handle.update_progress("late");
    tokio::time::sleep(Duration::from_millis(20)).await;
    let names: Vec<&str> = seen.lock().unwrap().iter().map(|(name, _)| *name).collect();
    assert_eq!(names, ["registered", "progress", "settled", "output"]);
    assert!(seen.lock().unwrap()[2].1, "a waiter was released");
}

#[tokio::test]
async fn teardown_cancels_waits_for_and_forgets_live_jobs() {
    let registry = Arc::new(Registry::default());
    let mut job = start(&registry, Some("a"));
    let closing = tokio::spawn({
        let registry = Arc::clone(&registry);
        async move { registry.dispose_owner("a").await }
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(
        *job.cancelled.lock().unwrap(),
        [Some("owner disposed".to_owned())]
    );
    job.end(ended(JobStatus::Killed, "gone"));
    closing.await.unwrap();
    assert!(registry.list(Some("a")).is_empty());
}

#[tokio::test]
async fn dropping_the_registry_cancels_what_still_runs() {
    let registry = Registry::default();
    let job = start(&registry, None);
    drop(registry);
    assert_eq!(job.cancelled.lock().unwrap().len(), 1);
}

struct Growing(Arc<Mutex<String>>);

impl JobOutputSource for Growing {
    fn channel(&self) -> Option<JobChannel> {
        Some(JobChannel::Stderr)
    }

    fn read(&self, from: u64) -> JobSourceRead {
        let text = self.0.lock().unwrap();
        let from = usize::try_from(from).unwrap();
        JobSourceRead {
            text: text[from..].to_owned(),
            next_offset: text.len() as u64,
            lossy: false,
            spill_path: Some("whole.log".to_owned()),
        }
    }
}

struct Broken;

impl JobOutputSource for Broken {
    fn read(&self, _from: u64) -> JobSourceRead {
        panic!("source failed")
    }
}

#[tokio::test]
async fn sources_are_drained_into_the_ring_even_when_one_fails() {
    let registry = Registry::default();
    let text = Arc::new(Mutex::new("first".to_owned()));
    let (mut spec, _, finish, _) = spec("bash", None);
    spec.output = vec![Box::new(Broken), Box::new(Growing(Arc::clone(&text)))];
    let id = registry.start(spec).unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    text.lock().unwrap().push_str(" last");
    finish.send(JobOutcome::completed()).unwrap();
    let view = settled(&registry, &id, None).await;
    assert_eq!(view.output.spill_paths, Some(vec!["whole.log".to_owned()]));
    let read = registry.read(&id, None).unwrap();
    assert_eq!(
        crate::jobs::delta(&read.chunks, read.lossy, None),
        "[stderr]\nfirst last"
    );
}

#[tokio::test]
async fn a_notice_says_how_it_ended_unless_the_model_already_knows() {
    let jobs = Jobs::default();
    let said: Arc<Mutex<Vec<String>>> = Arc::default();
    let into = Arc::clone(&said);
    let _subscription = jobs.on_notice(move |text| into.lock().unwrap().push(text));

    let mut quiet = start(jobs.registry(), None);
    quiet.handle.append("done\n", &JobAppendOptions::default());
    quiet.end(ended(JobStatus::Completed, "exit code: 0"));
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(
        said.lock().unwrap()[0],
        format!(
            "background job {} (bash: a job) finished [status: completed, exit code: 0]. Output:\ndone\n",
            quiet.id
        )
    );

    let mut long = start(jobs.registry(), None);
    long.handle
        .append("x".repeat(3000), &JobAppendOptions::default());
    long.end(JobOutcome::completed());
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(said.lock().unwrap()[1].ends_with("Read its output with job_output."));
    assert_eq!(
        jobs.registry().read(&long.id, None).unwrap().chunks.len(),
        1,
        "left unread"
    );

    let mut killed = start(jobs.registry(), None);
    let out = JobKill(jobs.clone())
        .execute(&json!({ "job_id": killed.id }), "")
        .await;
    assert_eq!(
        out.content,
        format!("requested cancellation of job {}", killed.id)
    );
    killed.end(ended(JobStatus::Killed, "gone"));
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(said.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn the_job_tools_report_output_status_and_the_list() {
    let jobs = Jobs::default();
    assert_eq!(
        JobList(jobs.clone()).execute(&json!({}), "").await.content,
        "(no background jobs)"
    );
    let mut job = start(jobs.registry(), None);
    let out = JobOutput(jobs.clone())
        .execute(&json!({ "job_id": job.id }), "")
        .await;
    assert_eq!(out.content, "(no new output)\n[status: running]");
    assert_eq!(
        JobList(jobs.clone()).execute(&json!({}), "").await.content,
        format!("{} [bash] running — a job", job.id)
    );
    job.handle.append("hello", &JobAppendOptions::default());
    job.end(ended(JobStatus::Completed, "exit code: 0"));
    let out = JobOutput(jobs.clone())
        .execute(&json!({ "job_id": job.id, "wait": true }), "")
        .await;
    assert_eq!(out.content, "hello\n[status: completed, exit code: 0]");
    let again = JobKill(jobs.clone())
        .execute(&json!({ "job_id": job.id }), "")
        .await;
    assert_eq!(
        again.content,
        format!(
            "job {} had already finished [status: completed, exit code: 0]",
            job.id
        )
    );
    assert!(
        JobOutput(jobs)
            .execute(&json!({ "job_id": "bash-0" }), "")
            .await
            .is_error
    );
}

#[tokio::test]
async fn the_person_speaking_ends_a_wait_but_not_the_job() {
    let jobs = Jobs::default();
    let job = start(jobs.registry(), None);
    let waiting = tokio::spawn({
        let (jobs, id) = (jobs.clone(), job.id.clone());
        async move {
            JobOutput(jobs)
                .execute(&json!({ "job_id": id, "wait": true }), "")
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    jobs.steered();
    let out = tokio::time::timeout(Duration::from_secs(5), waiting)
        .await
        .expect("released")
        .unwrap();
    assert_eq!(out.content, "(no new output)\n[status: running]");
}
