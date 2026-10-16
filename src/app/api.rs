//! Prompts from the API, launched the way the composer launches a task.

use std::time::{Duration, SystemTime};

use super::{App, Composer, Effect};
use crate::api::{Accepted, Answer, Refusal};
use crate::threads::LOCAL;

/// How long a prompt waits for the usage it asked for before Auto chooses
/// from the last report. Well inside the time the request waits for its
/// answer once the app has claimed it.
const USAGE_WAIT: Duration = Duration::from_secs(10);

/// A prompt held until the usage read it started comes back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    id: u64,
    repository: String,
    prompt: String,
    until: SystemTime,
}

/// Answers prompt `id`, or holds it while the usage Auto chooses from is
/// being read again: a report past its TTL would pick yesterday's harness.
pub(super) fn receive(
    app: &mut App,
    id: u64,
    repository: String,
    prompt: String,
    now: SystemTime,
    effects: &mut Vec<Effect>,
) {
    app.read_usage(false, now, effects);
    let refreshing = app.config.usage.enabled
        && app.usage_reading.contains(LOCAL)
        && !app.usage.get(LOCAL).is_some_and(|report| report.fresh(now));
    if refreshing && checked(app, &repository, &prompt, now, effects).is_ok() {
        app.api_held.push(Held { id, repository, prompt, until: now + USAGE_WAIT });
        return;
    }
    let answer = send(app, &repository, &prompt, now, effects);
    effects.push(Effect::ApiReply { id, answer });
}

/// Launches the held prompts once the usage they wait for is in, or once
/// they have waited long enough.
pub(super) fn release(app: &mut App, now: SystemTime, effects: &mut Vec<Effect>) {
    let read = !app.usage_reading.contains(LOCAL);
    let (due, held): (Vec<_>, Vec<_>) =
        std::mem::take(&mut app.api_held).into_iter().partition(|h| read || h.until <= now);
    app.api_held = held;
    for Held { id, repository, prompt, .. } in due {
        let answer = send(app, &repository, &prompt, now, effects);
        effects.push(Effect::ApiReply { id, answer });
    }
}

/// The project a prompt goes to, or why it can't go anywhere.
fn checked(
    app: &mut App,
    repository: &str,
    text: &str,
    now: SystemTime,
    effects: &mut Vec<Effect>,
) -> Result<String, Refusal> {
    if text.trim().is_empty() {
        return Err(Refusal::invalid("The prompt is empty."));
    }
    if !app.local().connection.is_live() {
        return Err(Refusal::unavailable("Herdr isn't running."));
    }
    let Some(project) = local_project(app, repository) else {
        // Projects are otherwise scanned when the composer opens, so a
        // checkout cloned since, or a first run, needs a scan of its own.
        let scanned = app.inventories.contains_key(LOCAL);
        app.discover(false, now, effects);
        return Err(if scanned {
            Refusal::not_found(format!("No local checkout of {repository}. Looking again: retry in a few seconds."))
        } else {
            Refusal::unavailable("Looking for projects. Retry in a few seconds.")
        });
    };
    project
}

/// Finds the local checkout of `repository` and sends `text` to it from a
/// fresh composer: the same harness choice (Auto when usage is read), model,
/// thinking, branch naming and history as a task typed in the inbox. The
/// composer on screen is left as it was.
fn send(app: &mut App, repository: &str, text: &str, now: SystemTime, effects: &mut Vec<Effect>) -> Answer {
    let project = checked(app, repository, text, now, effects)?;
    let mut composer = Composer {
        project: Some(project),
        machine: Some(LOCAL.to_string()),
        auto_harness: app.config.usage.enabled,
        ..Composer::default()
    };
    composer.set_task(text.trim());
    let shown = std::mem::replace(&mut app.composer, composer);
    app.with_composer(|composer, ctx| composer.settle(ctx));
    let start = effects.len();
    app.send(false, now, effects);
    let error = app.composer.error.take();
    app.composer = shown;
    let launched = effects[start..].iter().find_map(|effect| match effect {
        Effect::Launch { plan, .. } => Some(&plan.record),
        _ => None,
    });
    match launched {
        Some(record) => Ok(Accepted {
            harness: record.harness.clone(),
            branch: record.branch.clone(),
            agent: record.agent_name.clone(),
        }),
        None => Err(Refusal::invalid(error.unwrap_or_else(|| "Nothing was launched.".into()))),
    }
}

/// The project whose `origin` is `repository` on GitHub; of two clones, the
/// one named after it. The composer knows projects by name, so a clone whose
/// name another project also has is refused rather than launched elsewhere.
fn local_project(app: &App, repository: &str) -> Option<Result<String, Refusal>> {
    let inventory = app.inventories.get(LOCAL)?;
    let clones: Vec<_> = inventory.projects.iter().filter(|p| p.github.eq_ignore_ascii_case(repository)).collect();
    let name = repository.rsplit('/').next().unwrap_or(repository);
    let project = &clones.iter().find(|p| p.name.eq_ignore_ascii_case(name)).or(clones.first())?.name;
    if inventory.projects.iter().filter(|p| p.name == *project).count() > 1 {
        let message = format!("Another project is also named {project}: give one a name in [[projects]].");
        return Some(Err(Refusal::conflict(message)));
    }
    Some(Ok(project.clone()))
}
