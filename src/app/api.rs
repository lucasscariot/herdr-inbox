//! Prompts from the API, launched the way the composer launches a task.

use std::time::SystemTime;

use super::{App, Composer, Effect};
use crate::api::{Accepted, Answer, Refusal};
use crate::threads::LOCAL;

/// Finds the local checkout of `repository` and sends `text` to it from a
/// fresh composer: the same harness choice (Auto when usage is read), model,
/// thinking, branch naming and history as a task typed in the inbox. The
/// composer on screen is left as it was.
pub(super) fn prompt(
    app: &mut App,
    repository: &str,
    text: &str,
    now: SystemTime,
    effects: &mut Vec<Effect>,
) -> Answer {
    let text = text.trim();
    if text.is_empty() {
        return Err(Refusal::invalid("The prompt is empty."));
    }
    if !app.local().connection.is_live() {
        return Err(Refusal::unavailable("Herdr isn't running."));
    }
    // Auto chooses from the last report: keep it from going stale.
    app.read_usage(false, now, effects);
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
    let mut composer = Composer {
        project: Some(project),
        machine: Some(LOCAL.to_string()),
        auto_harness: app.config.usage.enabled,
        ..Composer::default()
    };
    composer.set_task(text);
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
/// one named after it.
fn local_project(app: &App, repository: &str) -> Option<String> {
    let inventory = app.inventories.get(LOCAL)?;
    let clones: Vec<_> = inventory.projects.iter().filter(|p| p.github.eq_ignore_ascii_case(repository)).collect();
    let name = repository.rsplit('/').next().unwrap_or(repository);
    clones.iter().find(|p| p.name.eq_ignore_ascii_case(name)).or(clones.first()).map(|p| p.name.clone())
}
