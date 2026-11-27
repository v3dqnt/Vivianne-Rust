//! The plan's acceptance scenarios (§16) for identity and membership, run through the service against any store.
//! Every backend runs this same list: [`all`].

#![allow(
    clippy::expect_used,
    clippy::missing_panics_doc,
    reason = "this module is a test suite"
)]

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::join::{self, JoinError};
use crate::membership::{RuleError, Status};
use crate::model::{JoinPolicy, Lifecycle, ProjectId, UserId, Visibility};
use crate::permission::{Denied, Role};
use crate::service::{Call, Error, NewProject, Projects};
use crate::store::{Guard, Store, StoreError, Write};

fn ids() -> String {
    static NEXT: AtomicUsize = AtomicUsize::new(1);
    format!("id-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

fn call(user: &str) -> Call {
    Call {
        user: user.into(),
        now: 1000,
        correlation: "c".into(),
    }
}

fn new(slug: &str, visibility: Visibility) -> NewProject {
    NewProject {
        slug: slug.to_owned(),
        name: slug.to_owned(),
        summary: String::new(),
        visibility,
        join_policy: JoinPolicy::Approval,
    }
}

fn service<S: Store>(store: &S) -> Projects<'_, S> {
    Projects { store, ids: &ids }
}

/// An active project owned by `owner`.
fn project<S: Store>(store: &S, visibility: Visibility) -> ProjectId {
    let made = service(store)
        .create(&call("owner"), new("main", visibility), None)
        .expect("created");
    service(store)
        .set_lifecycle(&call("owner"), &made.id, Lifecycle::Active)
        .expect("active");
    made.id
}

fn add<S: Store>(store: &S, id: &ProjectId, user: &str, role: Role) {
    service(store)
        .invite(&call("owner"), id, &user.into(), role)
        .expect("invited");
    service(store)
        .accept_invite(&call(user), id)
        .expect("accepted");
}

fn status<S: Store>(
    store: &S,
    id: &ProjectId,
    actor: &str,
    user: &str,
    to: Status,
    block: bool,
) -> Result<(), Error> {
    service(store)
        .set_member_status(&call(actor), id, &user.into(), to, block)
        .map(|_| ())
}

fn member<S: Store>(store: &S, id: &ProjectId, user: &str) -> Option<(Role, Status)> {
    store
        .member(id, &UserId::from(user))
        .expect("readable")
        .map(|m| (m.role, m.status))
}

fn asked_and_approved<S: Store>(store: &S) {
    let id = project(store, Visibility::Public);
    let projects = service(store);
    assert_eq!(projects.directory(None, 10).expect("listed").len(), 1);
    let request = projects
        .request_join(&call("ann"), &id, "I can help", Some("k1"))
        .expect("asked");
    assert_eq!(member(store, &id, "ann"), None, "asking grants nothing");
    let again = projects.request_join(&call("ann"), &id, "I can help", Some("k1"));
    assert_eq!(
        again,
        Ok(request.clone()),
        "a retried call makes nothing new"
    );
    assert_eq!(
        projects.request_join(&call("ann"), &id, "", None),
        Err(JoinError::Pending.into())
    );
    assert_eq!(
        projects
            .waiting_requests(&call("owner"), &id)
            .map(|r| r.len()),
        Ok(1)
    );
    projects
        .decide_join(&call("owner"), &request.id, 1, Some(Role::Contributor))
        .expect("approved");
    assert_eq!(
        member(store, &id, "ann"),
        Some((Role::Contributor, Status::Active))
    );
    assert_eq!(
        projects.decide_join(&call("owner"), &request.id, 1, None),
        Err(JoinError::Stale.into()),
        "the second reviewer loses"
    );
    let actions: Vec<_> = projects
        .audit_log(&call("owner"), &id)
        .expect("audit")
        .into_iter()
        .map(|e| e.action)
        .collect();
    assert_eq!(
        actions,
        [
            "project.create",
            "project.lifecycle",
            "join.request",
            "join.decide"
        ]
    );
    assert!(projects.audit_log(&call("ann"), &id).is_err());
    assert_eq!(
        projects.members(&call("ann"), &id, None).map(|m| m.len()),
        Ok(2)
    );
}

fn private_looks_missing<S: Store>(store: &S) {
    let id = project(store, Visibility::Private);
    let projects = service(store);
    assert_eq!(projects.directory(None, 10).map(|d| d.len()), Ok(0));
    let missing = projects.view(&call("eve"), &"nope".into());
    assert_eq!(projects.view(&call("eve"), &id), missing);
    assert_eq!(
        projects.request_join(&call("eve"), &id, "", None),
        Err(JoinError::NotFound.into())
    );
    assert_eq!(
        projects.change_role(&call("eve"), &id, &"owner".into(), Role::Viewer),
        Err(Error::NotFound)
    );
    assert_eq!(
        projects.members(&call("eve"), &id, None),
        Err(Error::NotFound)
    );
    let other = projects
        .create(&call("bob"), new("bobs", Visibility::Private), None)
        .expect("bob's");
    assert_eq!(projects.view(&call("bob"), &id), Err(Error::NotFound));
    assert_eq!(
        projects.view(&call("owner"), &other.id),
        Err(Error::NotFound)
    );
}

fn no_escalation<S: Store>(store: &S) {
    let id = project(store, Visibility::Private);
    add(store, &id, "admin", Role::Admin);
    add(store, &id, "dev", Role::Contributor);
    let projects = service(store);
    assert_eq!(
        projects.change_role(&call("admin"), &id, &"admin".into(), Role::Owner),
        Err(RuleError::OwnRole.into())
    );
    assert_eq!(
        projects.change_role(&call("dev"), &id, &"dev".into(), Role::Admin),
        Err(Denied::Role.into())
    );
    assert_eq!(
        projects.invite(&call("admin"), &id, &"friend".into(), Role::Admin),
        Err(RuleError::Rank.into())
    );
    for actor in ["admin", "owner"] {
        assert_eq!(
            status(store, &id, actor, "owner", Status::Removed, false),
            Err(RuleError::FinalOwner.into()),
            "{actor}"
        );
    }
}

fn removal_sticks<S: Store>(store: &S) {
    let id = project(store, Visibility::Public);
    add(store, &id, "dev", Role::Contributor);
    status(store, &id, "owner", "dev", Status::Suspended, false).expect("suspended");
    assert_eq!(
        status(store, &id, "dev", "dev", Status::Removed, false),
        Err(Denied::Inactive.into()),
        "leaving does not shed a suspension"
    );
    status(store, &id, "owner", "dev", Status::Removed, true).expect("removed");
    assert_eq!(
        service(store).request_join(&call("dev"), &id, "", None),
        Err(JoinError::Blocked.into())
    );
    assert!(status(store, &id, "owner", "dev", Status::Active, false).is_err());
    let log = service(store)
        .audit_log(&call("owner"), &id)
        .expect("audit");
    assert!(log.iter().any(|e| e.resource == "dev"), "history survives");
}

fn lifecycle<S: Store>(store: &S) {
    let id = project(store, Visibility::Public);
    add(store, &id, "admin", Role::Admin);
    let projects = service(store);
    let request = projects
        .request_join(&call("ann"), &id, "", None)
        .expect("asked");
    for _ in 0..2 {
        projects
            .set_lifecycle(&call("owner"), &id, Lifecycle::Archived)
            .expect("archived");
    }
    let closed = store.join(&request.id).expect("readable").map(|j| j.state);
    assert_eq!(closed, Some(join::State::Cancelled));
    assert_eq!(projects.directory(None, 10).map(|d| d.len()), Ok(0));
    assert!(
        projects
            .set_lifecycle(&call("owner"), &id, Lifecycle::Suspended)
            .is_err()
    );
    assert!(
        projects
            .set_visibility(&call("owner"), &id, Visibility::Public)
            .is_err()
    );
    projects
        .set_visibility(&call("admin"), &id, Visibility::Private)
        .expect("taken out of view");
    projects
        .set_lifecycle(&call("owner"), &id, Lifecycle::DeletionPending)
        .expect("deletion asked for");
    assert!(
        projects
            .set_lifecycle(&call("admin"), &id, Lifecycle::Archived)
            .is_err(),
        "only the owner takes a deletion back"
    );
}

fn ownership<S: Store>(store: &S) {
    let id = project(store, Visibility::Private);
    add(store, &id, "next", Role::Admin);
    let projects = service(store);
    projects
        .offer_ownership(&call("owner"), &id, &"next".into(), true)
        .expect("offered");
    assert_eq!(member(store, &id, "owner").map(|m| m.0), Some(Role::Owner));
    projects
        .accept_ownership(&call("next"), &id)
        .expect("accepted");
    assert_eq!(member(store, &id, "owner").map(|m| m.0), Some(Role::Admin));
    assert_eq!(member(store, &id, "next").map(|m| m.0), Some(Role::Owner));
}

fn requests_and_memberships<S: Store>(store: &S) {
    let id = project(store, Visibility::Public);
    add(store, &id, "dev", Role::Contributor);
    let projects = service(store);
    let asked = projects
        .request_join(&call("ann"), &id, "", None)
        .expect("asked");
    add(store, &id, "ann", Role::Viewer);
    let closed = store.join(&asked.id).expect("readable").map(|j| j.state);
    assert_eq!(closed, Some(join::State::Cancelled));

    status(store, &id, "owner", "dev", Status::Suspended, false).expect("suspended");
    projects
        .invite(&call("owner"), &id, &"inv".into(), Role::Viewer)
        .expect("invited");
    let current = store.project(&id).expect("readable").expect("exists");
    for (request, user) in [("stale-a", "dev"), ("stale-b", "inv")] {
        // A request left over from before the membership, however it got there.
        let stale = join::new_request(&current, request, user.into(), "", 1000).expect("made");
        store
            .commit(Vec::new(), vec![Write::Join(stale)], Vec::new())
            .expect("stored");
        assert_eq!(
            projects.decide_join(&call("owner"), request, 1, Some(Role::Maintainer)),
            Err(JoinError::AlreadyMember.into()),
            "{user}"
        );
    }
    assert!(
        projects.view(&call("inv"), &id).is_ok(),
        "an invitee may look"
    );
    assert_eq!(
        projects.invite(&call("inv"), &id, &"x".into(), Role::Viewer),
        Err(Denied::Inactive.into())
    );
}

fn races_and_refusals<S: Store>(store: &S) {
    let id = project(store, Visibility::Public);
    add(store, &id, "admin", Role::Admin);
    let current = store.project(&id).expect("readable").expect("exists");
    let make = |r| join::new_request(&current, r, "ann".into(), "", 1000).expect("made");
    store
        .commit(Vec::new(), vec![Write::Join(make("twin-a"))], Vec::new())
        .expect("first");
    assert_eq!(
        store.commit(Vec::new(), vec![Write::Join(make("twin-b"))], Vec::new()),
        Err(StoreError::Conflict)
    );

    let before = store
        .member(&id, &"admin".into())
        .expect("readable")
        .expect("member");
    status(store, &id, "owner", "admin", Status::Removed, false).expect("removed");
    let guard = Guard::Member(id.clone(), before.user.clone(), before.revision);
    assert_eq!(
        store.commit(vec![guard], Vec::new(), Vec::new()),
        Err(StoreError::Conflict),
        "a write decided on old facts is refused"
    );

    let taken = service(store).create(&call("bob"), new("main", Visibility::Public), Some("k"));
    assert_eq!(taken, Err(StoreError::SlugTaken.into()));
    assert_eq!(
        store.key(&"bob".into(), "k"),
        Ok(None),
        "a refused batch leaves nothing"
    );
    let first = service(store)
        .create(&call("bob"), new("bobs", Visibility::Private), Some("k"))
        .expect("made");
    let again = service(store).create(&call("bob"), new("bobs", Visibility::Private), Some("k"));
    assert_eq!(again, Ok(first));
    assert_eq!(
        service(store).create(&call("bob"), new("xyz", Visibility::Private), Some("../x")),
        Err(Error::Key)
    );
}

fn pages<S: Store>(store: &S) {
    for slug in ["ccc", "aaa", "bbb"] {
        let made = service(store)
            .create(&call("owner"), new(slug, Visibility::Public), None)
            .expect("made");
        service(store)
            .set_lifecycle(&call("owner"), &made.id, Lifecycle::Active)
            .expect("active");
    }
    let slugs = |after| {
        service(store)
            .directory(after, 2)
            .expect("listed")
            .into_iter()
            .map(|p| p.slug)
            .collect::<Vec<_>>()
    };
    assert_eq!(slugs(None), ["aaa", "bbb"]);
    assert_eq!(slugs(Some("bbb")), ["ccc"]);
}

fn expired_requests_leave_the_queue<S: Store>(store: &S) {
    let id = project(store, Visibility::Public);
    let projects = service(store);
    let old = projects
        .request_join(&call("ann"), &id, "", None)
        .expect("asked");
    let later = |user| Call {
        now: old.expires_at,
        ..call(user)
    };
    assert_eq!(
        projects.waiting_requests(&later("owner"), &id),
        Ok(Vec::new())
    );
    assert_eq!(store.pending_joins(&id, old.expires_at, 10), Ok(Vec::new()));
    let fresh = projects
        .request_join(&later("ann"), &id, "", None)
        .expect("may ask again");
    assert_eq!(
        projects.waiting_requests(&later("owner"), &id),
        Ok(vec![fresh])
    );
}

/// Runs every scenario, each on a fresh store.
pub fn all<S: Store>(fresh: impl Fn() -> S) {
    asked_and_approved(&fresh());
    private_looks_missing(&fresh());
    no_escalation(&fresh());
    removal_sticks(&fresh());
    lifecycle(&fresh());
    ownership(&fresh());
    requests_and_memberships(&fresh());
    races_and_refusals(&fresh());
    pages(&fresh());
    expired_requests_leave_the_queue(&fresh());
}
