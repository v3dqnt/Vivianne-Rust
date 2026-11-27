//! Following a started run to its end, and starting one when a background job finishes while the thread is idle.

use std::time::Duration;

use super::session::Run;
use super::*;

/// How often an idle thread with running jobs looks for one that ended.
const JOB_CHECK: Duration = Duration::from_secs(1);

impl ChatView {
    pub(super) fn follow(&mut self, run: Run, window: &mut Window, cx: &mut Context<Self>) {
        self.running = true;
        let Run { mut events, done } = run;
        cx.spawn_in(window, async move |this, cx| {
            while let Some(first) = events.recv().await {
                let mut batch = vec![first];
                while let Ok(more) = events.try_recv() {
                    batch.push(more);
                }
                if this
                    .update(cx, |chat, cx| chat.apply_events(&batch, cx))
                    .is_err()
                {
                    return;
                }
                cx.background_executor().timer(EVENT_BATCH_INTERVAL).await;
            }
            // No answer means the run's task died; the chat must not stay "working" for ever.
            let finished = done.await.ok();
            this.update_in(cx, |chat, window, cx| {
                chat.finish(finished, cx);
                chat.next_step(window, cx);
                chat.watch_jobs(window, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// While the thread is idle and has background jobs, wakes the agent when one ends. A run that starts meanwhile
    /// takes the notices itself, and watches again when it ends.
    fn watch_jobs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(agent) = self.session.as_ref().ok().and_then(Session::agent) else {
            return;
        };
        if self.running || !(agent.background_running() || agent.has_steering()) {
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            // Two quiet looks before giving up: a job settles an instant before its notice arrives.
            let mut quiet = 0;
            while quiet < 2 {
                cx.background_executor().timer(JOB_CHECK).await;
                if agent.has_steering() {
                    this.update_in(cx, |chat, window, cx| chat.wake(&agent, window, cx))
                        .ok();
                    return;
                }
                quiet = if agent.background_running() {
                    0
                } else {
                    quiet + 1
                };
                if this.update(cx, |chat, _| chat.running).unwrap_or(true) {
                    return;
                }
            }
        })
        .detach();
    }

    /// Starts a run with what the jobs had to say. The message box, and anything typed in it, is left alone.
    fn wake(
        &mut self,
        agent: &std::sync::Arc<vivianne_runtime::Agent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.running {
            return;
        }
        let Ok(session) = self.session.as_mut() else {
            return;
        };
        let Some(text) = agent.take_waiting() else {
            return;
        };
        let Some(run) = session.send(text.clone(), self.composer.thinking) else {
            // Another Vivianne is writing this conversation: the notice waits for the next message.
            agent.steer(text);
            return;
        };
        self.push_item(Item::Notice(text), cx);
        self.follow(run, window, cx);
    }
}
