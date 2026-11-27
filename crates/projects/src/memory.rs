//! The in-memory store: the reference for what a backend must do, and what the tests run against.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, PoisonError};

use crate::audit;
use crate::join::{JoinRequest, State};
use crate::membership::{Membership, Status, Transfer};
use crate::model::{Lifecycle, Project, ProjectId, UserId, Visibility};
use crate::store::{Guard, MAX_PAGE, Store, StoreError, Stored, Write};

#[derive(Debug, Default, Clone)]
struct Data {
    projects: BTreeMap<ProjectId, Project>,
    members: BTreeMap<(ProjectId, UserId), Membership>,
    joins: BTreeMap<String, JoinRequest>,
    transfers: BTreeMap<ProjectId, Transfer>,
    blocks: BTreeSet<(ProjectId, UserId)>,
    keys: BTreeMap<(UserId, String), String>,
    events: Vec<audit::Event>,
}

fn follows(stored: Option<u64>, written: u64) -> Stored<()> {
    if stored.map_or(1, |r| r + 1) == written {
        Ok(())
    } else {
        Err(StoreError::Conflict)
    }
}

impl Data {
    fn count(&self, project: &ProjectId) -> usize {
        self.members
            .values()
            .filter(|m| m.project == *project && m.status != Status::Removed)
            .count()
    }

    fn holds(&self, guard: &Guard) -> bool {
        match guard {
            Guard::Project(id, revision) => self
                .projects
                .get(id)
                .is_some_and(|p| p.revision == *revision),
            Guard::Member(project, user, revision) => self
                .members
                .get(&(project.clone(), user.clone()))
                .is_some_and(|m| m.revision == *revision),
            Guard::Members(project, count) => self.count(project) == *count,
        }
    }

    fn apply(&mut self, write: Write) -> Stored<()> {
        match write {
            Write::Project(p) => {
                follows(self.projects.get(&p.id).map(|x| x.revision), p.revision)?;
                if self
                    .projects
                    .values()
                    .any(|x| x.slug == p.slug && x.id != p.id)
                {
                    return Err(StoreError::SlugTaken);
                }
                self.projects.insert(p.id.clone(), p);
            }
            Write::Member(m) => {
                let key = (m.project.clone(), m.user.clone());
                follows(self.members.get(&key).map(|x| x.revision), m.revision)?;
                self.members.insert(key, m);
            }
            Write::Join(j) => {
                follows(self.joins.get(&j.id).map(|x| x.revision), j.revision)?;
                // One live request per person and project, even when two are made at once.
                let twin = j.state == State::Pending
                    && self.joins.values().any(|x| {
                        x.id != j.id
                            && x.project == j.project
                            && x.user == j.user
                            && x.state == State::Pending
                            && x.expires_at > j.created_at
                    });
                if twin {
                    return Err(StoreError::Conflict);
                }
                self.joins.insert(j.id.clone(), j);
            }
            Write::Transfer(project, Some(transfer)) => {
                self.transfers.insert(project, transfer);
            }
            Write::Transfer(project, None) => {
                self.transfers.remove(&project);
            }
            Write::Block(project, user) => {
                self.blocks.insert((project, user));
            }
            Write::Key {
                user,
                key,
                resource,
            } => {
                if self.keys.insert((user, key), resource).is_some() {
                    return Err(StoreError::Conflict);
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct MemoryStore(Mutex<Data>);

impl MemoryStore {
    #[allow(clippy::unnecessary_wraps, reason = "every read of the trait can fail")]
    fn read<T>(&self, f: impl FnOnce(&Data) -> T) -> Stored<T> {
        Ok(f(&self.0.lock().unwrap_or_else(PoisonError::into_inner)))
    }
}

impl Store for MemoryStore {
    fn project(&self, id: &ProjectId) -> Stored<Option<Project>> {
        self.read(|d| d.projects.get(id).cloned())
    }

    fn public_projects(&self, after: Option<&str>, limit: usize) -> Stored<Vec<Project>> {
        self.read(|d| {
            let mut found: Vec<Project> = d
                .projects
                .values()
                .filter(|p| p.lifecycle == Lifecycle::Active && p.visibility == Visibility::Public)
                .filter(|p| after.is_none_or(|a| p.slug.as_str() > a))
                .cloned()
                .collect();
            found.sort_by(|a, b| a.slug.cmp(&b.slug));
            found.truncate(limit.min(MAX_PAGE));
            found
        })
    }

    fn member(&self, project: &ProjectId, user: &UserId) -> Stored<Option<Membership>> {
        self.read(|d| d.members.get(&(project.clone(), user.clone())).cloned())
    }

    fn member_count(&self, project: &ProjectId) -> Stored<usize> {
        self.read(|d| d.count(project))
    }

    fn members(
        &self,
        project: &ProjectId,
        after: Option<&UserId>,
        limit: usize,
    ) -> Stored<Vec<Membership>> {
        self.read(|d| {
            d.members
                .values()
                .filter(|m| m.project == *project && after.is_none_or(|a| m.user > *a))
                .take(limit.min(MAX_PAGE))
                .cloned()
                .collect()
        })
    }

    fn join(&self, id: &str) -> Stored<Option<JoinRequest>> {
        self.read(|d| d.joins.get(id).cloned())
    }

    fn joins_of(&self, project: &ProjectId, user: &UserId) -> Stored<Vec<JoinRequest>> {
        self.read(|d| {
            d.joins
                .values()
                .filter(|j| j.project == *project && j.user == *user)
                .cloned()
                .collect()
        })
    }

    fn pending_joins(
        &self,
        project: &ProjectId,
        now: u64,
        limit: usize,
    ) -> Stored<Vec<JoinRequest>> {
        self.read(|d| {
            let mut found: Vec<JoinRequest> = d
                .joins
                .values()
                .filter(|j| {
                    j.project == *project && j.state == State::Pending && j.expires_at > now
                })
                .cloned()
                .collect();
            found.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
            found.truncate(limit);
            found
        })
    }

    fn transfer(&self, project: &ProjectId) -> Stored<Option<Transfer>> {
        self.read(|d| d.transfers.get(project).cloned())
    }

    fn is_blocked(&self, project: &ProjectId, user: &UserId) -> Stored<bool> {
        self.read(|d| d.blocks.contains(&(project.clone(), user.clone())))
    }

    fn audit(&self, project: &ProjectId, limit: usize) -> Stored<Vec<audit::Event>> {
        self.read(|d| {
            let mut found: Vec<audit::Event> = d
                .events
                .iter()
                .rev()
                .filter(|e| e.project == *project)
                .take(limit.min(MAX_PAGE))
                .cloned()
                .collect();
            found.reverse();
            found
        })
    }

    fn key(&self, user: &UserId, key: &str) -> Stored<Option<String>> {
        self.read(|d| d.keys.get(&(user.clone(), key.to_owned())).cloned())
    }

    fn commit(
        &self,
        guards: Vec<Guard>,
        writes: Vec<Write>,
        events: Vec<audit::Event>,
    ) -> Stored<()> {
        let mut data = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if !guards.iter().all(|g| data.holds(g)) {
            return Err(StoreError::Conflict);
        }
        // Applied to a copy first, so a refused write leaves nothing behind.
        let mut next = data.clone();
        for write in writes {
            next.apply(write)?;
        }
        next.events.extend(events);
        *data = next;
        Ok(())
    }
}
