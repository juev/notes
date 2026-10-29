//! R131: the history of undo. An action that changes a task remembers, for every register it
//! writes, what the register held before and the stamp it was given; undo writes the former values
//! back as an ordinary local change, so it syncs like any other. The history lives in memory, one
//! stack per actor: the apps are one actor, the web server makes each session one (R131).

use std::cell::RefCell;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

use crate::db::KIND_TASK;
use crate::error::Result;
use crate::store::{Store, Writer};

/// How many actions a history keeps; older ones fall out.
const DEPTH: usize = 100;

thread_local! {
    /// The actor the calls of this thread act for (see `Store::as_actor`).
    static ACTOR: RefCell<String> = const { RefCell::new(String::new()) };
}

fn actor() -> String {
    ACTOR.with(|a| a.borrow().clone())
}

/// One register an action wrote.
struct Register {
    kind: String,
    id: String,
    field: String,
    /// The value and stamp before the action; `None` when the register was not there.
    before: Option<(String, String)>,
    /// The stamp the action left on it.
    written: String,
}

/// What one action did to one task, with its subtasks and the record of a repeat: undone whole or
/// not at all (R131).
#[derive(Default)]
pub(crate) struct Step {
    registers: Vec<Register>,
    /// The objects the action wrote, and whether each was there before it.
    existed: HashMap<(String, String), bool>,
    /// Edits of the same text of the same task in a row make one step (R131).
    text: Option<(String, String)>,
}

impl Step {
    /// Called before a register is written, with the stamp it is about to get.
    pub(crate) fn note(&mut self, conn: &Connection, kind: &str, id: &str, field: &str, stamp: &str) -> Result<()> {
        if let Entry::Vacant(object) = self.existed.entry((kind.to_string(), id.to_string())) {
            object.insert(conn.query_row(
                "SELECT EXISTS (SELECT 1 FROM fields WHERE kind = ?1 AND id = ?2)",
                params![kind, id],
                |r| r.get(0),
            )?);
        }
        if let Some(known) = self
            .registers
            .iter_mut()
            .find(|r| r.kind == kind && r.id == id && r.field == field)
        {
            known.written = stamp.to_string();
            return Ok(());
        }
        let before = conn
            .query_row(
                "SELECT value, stamp FROM fields WHERE kind = ?1 AND id = ?2 AND field = ?3",
                params![kind, id, field],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        self.registers.push(Register {
            kind: kind.into(),
            id: id.into(),
            field: field.into(),
            before,
            written: stamp.into(),
        });
        Ok(())
    }

    /// Whether nothing has written over the step since: by this device or another one, by sync or CalDAV.
    fn current(&self, conn: &Connection) -> Result<bool> {
        for r in &self.registers {
            let stamp: Option<String> = conn
                .query_row(
                    "SELECT stamp FROM fields WHERE kind = ?1 AND id = ?2 AND field = ?3",
                    params![r.kind, r.id, r.field],
                    |row| row.get(0),
                )
                .optional()?;
            if stamp.as_deref() != Some(r.written.as_str()) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Writes back what the step changed. Returns the registers written, with the stamp each had
    /// before and has now, so that the steps under it can follow (see `History::restamp`).
    fn revert(&self, w: &mut Writer) -> Result<Vec<(String, String)>> {
        let mut moved = Vec::new();
        let mut gone = HashSet::new();
        for ((kind, id), existed) in &self.existed {
            if *existed {
                continue;
            }
            gone.insert((kind.as_str(), id.as_str()));
            let fields: Vec<&str> = self
                .registers
                .iter()
                .filter(|r| &r.kind == kind && &r.id == id)
                .map(|r| r.field.as_str())
                .collect();
            // A record of a repeat that was not there is no longer completed, as R128 takes it back;
            // anything else the action made goes to the trash.
            if kind == KIND_TASK && fields.contains(&"log_of") {
                for field in ["done", "wont"].into_iter().filter(|f| fields.contains(f)) {
                    w.set(kind, id, field, Value::Null)?;
                }
            } else {
                w.set(kind, id, "deleted", json!(true))?;
            }
        }
        for r in self.registers.iter().rev() {
            if gone.contains(&(r.kind.as_str(), r.id.as_str())) {
                continue;
            }
            let value = match &r.before {
                Some((value, _)) => serde_json::from_str(value).unwrap_or(Value::Null),
                None => Value::Null,
            };
            let stamp = w.set_stamped(&r.kind, &r.id, &r.field, value)?;
            if let Some((_, before)) = &r.before {
                moved.push((before.clone(), stamp));
            }
        }
        w.flush()?;
        Ok(moved)
    }
}

/// The steps one undo takes back: one for an action on one task, several for an action on many.
type Action = Vec<Step>;

#[derive(Default)]
pub(crate) struct History {
    stacks: HashMap<String, Vec<Action>>,
    /// Actions on several tasks being put together (`Store::begin_group`).
    open: HashMap<String, Action>,
    /// How many actions each history has taken in (`Store::undo_revision`).
    revisions: HashMap<String, u64>,
}

impl History {
    fn push(&mut self, actor: String, step: Step) {
        match self.open.get_mut(&actor) {
            Some(group) => group.push(step),
            None => self.add(actor, vec![step]),
        }
    }

    /// Puts an action on top of the history, or into the one on top when both edit the same text.
    fn add(&mut self, actor: String, mut action: Action) {
        let stack = self.stacks.entry(actor.clone()).or_default();
        if let ([step], Some([last])) = (action.as_mut_slice(), stack.last_mut().map(Vec::as_mut_slice)) {
            if step.text.is_some() && last.text == step.text {
                for r in std::mem::take(&mut step.registers) {
                    match last
                        .registers
                        .iter_mut()
                        .find(|k| k.kind == r.kind && k.id == r.id && k.field == r.field)
                    {
                        Some(known) => known.written = r.written,
                        None => last.registers.push(r),
                    }
                }
                return;
            }
        }
        stack.push(action);
        if stack.len() > DEPTH {
            stack.remove(0);
        }
        *self.revisions.entry(actor).or_default() += 1;
    }

    /// A register undo wrote has a new stamp: a step lower in any history that left the old one
    /// is still current, since what it wrote is there again.
    fn restamp(&mut self, moved: &[(String, String)]) {
        let moved: HashMap<&str, &str> = moved.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        for step in self.stacks.values_mut().flatten().flatten() {
            for r in &mut step.registers {
                if let Some(now) = moved.get(r.written.as_str()) {
                    r.written = (*now).to_string();
                }
            }
        }
    }
}

impl Store {
    /// Runs `f` with the calls it makes acting for `actor`, so that they go into its own history.
    /// The web server gives each session one (R131). Calls outside act for the app itself.
    pub fn as_actor<T>(&self, actor: &str, f: impl FnOnce() -> T) -> T {
        let before = ACTOR.with(|a| std::mem::replace(&mut *a.borrow_mut(), actor.to_string()));
        let out = f();
        ACTOR.with(|a| *a.borrow_mut() = before);
        out
    }

    /// A write that the history can take back: one step of the current actor (R131).
    pub(crate) fn act<T>(&self, f: impl FnOnce(&mut Writer) -> Result<T>) -> Result<T> {
        self.act_on(None, f)
    }

    /// The same for an edit of a text of the task; edits in a row make one step.
    pub(crate) fn act_on<T>(
        &self,
        text: Option<(String, String)>,
        f: impl FnOnce(&mut Writer) -> Result<T>,
    ) -> Result<T> {
        let (out, step) = self.lock().write_noted(f)?;
        if !step.registers.is_empty() {
            let step = Step { text, ..step };
            self.history().push(actor(), step);
        }
        Ok(out)
    }

    fn history(&self) -> std::sync::MutexGuard<'_, History> {
        self.history.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[uniffi::export]
impl Store {
    /// R131: takes back the last action of this history that is not taken back yet. A task that has
    /// changed since is left as it is. Returns how many tasks came back.
    pub fn undo(&self) -> Result<u32> {
        let actor = actor();
        let Some(action) = self.history().stacks.get_mut(&actor).and_then(Vec::pop) else {
            return Ok(0);
        };
        let (count, moved) = self.write(|w| {
            let (mut count, mut moved) = (0, Vec::new());
            for step in action.iter().rev() {
                if step.current(w.tx)? {
                    moved.extend(step.revert(w)?);
                    count += 1;
                }
            }
            Ok((count, moved))
        })?;
        self.history().restamp(&moved);
        Ok(count)
    }

    /// Whether there is an action to take back (R131).
    pub fn can_undo(&self) -> bool {
        self.history().stacks.get(&actor()).is_some_and(|s| !s.is_empty())
    }

    /// The actions until `end_group` are taken back by one undo: an action on several tasks (R131).
    pub fn begin_group(&self) {
        self.history().open.insert(actor(), Vec::new());
    }

    pub fn end_group(&self) {
        let mut history = self.history();
        let actor = actor();
        if let Some(group) = history.open.remove(&actor).filter(|g| !g.is_empty()) {
            history.add(actor, group);
        }
    }

    /// Grows by one with every action that goes into this history, and stays when an edit of a text
    /// joins the one before. An app that keeps an undo stack of its own (macOS) compares it before
    /// and after a change to know whether to put a step there.
    pub fn undo_revision(&self) -> u64 {
        self.history().revisions.get(&actor()).copied().unwrap_or(0)
    }
}
