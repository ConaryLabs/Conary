// conary-core/src/db/models/trigger_engine.rs

//! Trigger execution and scheduling helpers.
//!
//! Keeping the execution engine separate from the row-model types in
//! `trigger.rs` makes the persistence layer easier to scan and gives the
//! ordering logic a dedicated home.

use super::{ChangesetTrigger, Trigger, TriggerDependency};
use crate::error::Result;
use rusqlite::Connection;
use std::collections::{HashMap, VecDeque};
use tracing::{debug, warn};

/// Trigger execution engine.
pub struct TriggerEngine<'a> {
    conn: &'a Connection,
}

impl<'a> TriggerEngine<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// Find all triggers that match the given file paths.
    pub fn find_matching_triggers(
        &self,
        file_paths: &[String],
    ) -> Result<Vec<(Trigger, Vec<String>)>> {
        let triggers = Trigger::list_enabled(self.conn)?;
        let mut matches: HashMap<i64, (Trigger, Vec<String>)> = HashMap::new();

        for path in file_paths {
            for trigger in &triggers {
                if trigger.matches(path) {
                    let trigger_id = trigger.id.unwrap_or(0);
                    matches
                        .entry(trigger_id)
                        .or_insert_with(|| (trigger.clone(), Vec::new()))
                        .1
                        .push(path.clone());
                }
            }
        }

        Ok(matches.into_values().collect())
    }

    /// Record triggered handlers for a changeset.
    pub fn record_triggers(
        &self,
        changeset_id: i64,
        file_paths: &[String],
    ) -> Result<Vec<Trigger>> {
        let matching = self.find_matching_triggers(file_paths)?;
        let mut triggered = Vec::new();

        for (trigger, matched_files) in matching {
            if let Some(trigger_id) = trigger.id {
                let mut ct = ChangesetTrigger::new(changeset_id, trigger_id);
                ct.matched_files = matched_files.len() as i32;
                ct.upsert(self.conn)?;

                debug!(
                    "Trigger '{}' matched {} files",
                    trigger.name,
                    matched_files.len()
                );
                triggered.push(trigger);
            }
        }

        Ok(triggered)
    }

    /// Get triggers for a changeset in execution order (topologically sorted).
    pub fn get_execution_order(&self, changeset_id: i64) -> Result<Vec<Trigger>> {
        let changeset_triggers = ChangesetTrigger::find_pending(self.conn, changeset_id)?;
        if changeset_triggers.is_empty() {
            return Ok(Vec::new());
        }

        let trigger_ids: Vec<i64> = changeset_triggers.iter().map(|ct| ct.trigger_id).collect();
        let mut triggers: HashMap<String, Trigger> = Trigger::find_by_ids(self.conn, &trigger_ids)?
            .into_iter()
            .map(|t| (t.name.clone(), t))
            .collect();

        let loaded_ids: Vec<i64> = triggers.values().filter_map(|t| t.id).collect();
        let all_deps = TriggerDependency::get_dependencies_batch(self.conn, &loaded_ids)?;

        let mut in_degree: HashMap<String, usize> = HashMap::new();
        let mut dependents: HashMap<String, Vec<String>> = HashMap::new();

        for trigger in triggers.values() {
            in_degree.entry(trigger.name.clone()).or_insert(0);
            dependents.entry(trigger.name.clone()).or_default();
        }

        for trigger in triggers.values() {
            let trigger_id = match trigger.id {
                Some(id) => id,
                None => continue,
            };
            let deps = all_deps
                .get(&trigger_id)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            for dep in deps {
                if triggers.contains_key(dep.as_str()) {
                    *in_degree.entry(trigger.name.clone()).or_insert(0) += 1;
                    dependents
                        .entry(dep.clone())
                        .or_default()
                        .push(trigger.name.clone());
                }
            }
        }

        let mut sorted = Vec::new();
        let mut ready: Vec<String> = in_degree
            .iter()
            .filter(|&(_, &degree)| degree == 0)
            .map(|(name, _)| name.clone())
            .collect();
        ready.sort_by(|a, b| {
            let pa = triggers.get(a).map_or(i32::MAX, |t| t.priority);
            let pb = triggers.get(b).map_or(i32::MAX, |t| t.priority);
            pa.cmp(&pb).then(a.cmp(b))
        });
        let mut queue: VecDeque<String> = ready.into_iter().collect();

        while let Some(name) = queue.pop_front() {
            let mut newly_ready = Vec::new();

            if let Some(deps) = dependents.get(&name) {
                for dependent in deps {
                    if let Some(degree) = in_degree.get_mut(dependent) {
                        *degree -= 1;
                        if *degree == 0 {
                            newly_ready.push(dependent.clone());
                        }
                    }
                }
            }

            if let Some(trigger) = triggers.remove(&name) {
                sorted.push(trigger);
            }

            newly_ready.sort_by(|a, b| {
                let pa = triggers.get(a).map_or(i32::MAX, |t| t.priority);
                let pb = triggers.get(b).map_or(i32::MAX, |t| t.priority);
                pa.cmp(&pb).then(a.cmp(b))
            });
            for ready_name in newly_ready {
                queue.push_back(ready_name);
            }
        }

        if sorted.len() != changeset_triggers.len() {
            warn!("Circular dependency detected in triggers, using priority order fallback");
            let mut remaining: Vec<Trigger> = triggers.into_values().collect();
            remaining.sort_by(|a, b| a.priority.cmp(&b.priority));
            sorted.extend(remaining);
        }

        Ok(sorted)
    }
}
