//! The HTTP API without the sockets: a request in, a status and JSON out. Who is asking comes only from the token;
//! a `user` or `role` a body claims for the caller is not read. A project the caller may not see answers exactly
//! like one that does not exist.

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use vivianne_projects::join::JoinError;
use vivianne_projects::membership::{RuleError, Status};
use vivianne_projects::model::{JoinPolicy, Lifecycle, Project, ProjectId, UserId, Visibility};
use vivianne_projects::permission::Role;
use vivianne_projects::service::{Call, Error, NewProject, Projects};
use vivianne_projects::store::{Store as _, StoreError};

use crate::sessions::{random_hex, valid_user};
use crate::sqlite::Database;

#[derive(Debug, Clone, Copy, Default)]
pub struct Request<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub query: &'a str,
    /// The token from the `Authorization` header. Never taken from the address.
    pub bearer: Option<&'a str>,
    pub idempotency_key: Option<&'a str>,
    pub body: &'a [u8],
}

#[derive(Debug, PartialEq)]
pub struct Response {
    pub status: u16,
    pub body: Value,
}

fn reply(status: u16, body: Value) -> Response {
    Response { status, body }
}

fn failure(status: u16, message: &str) -> Response {
    reply(status, json!({ "error": message }))
}

/// No operation makes more records with new ids than this.
const IDS_PER_CALL: usize = 1;
const NOT_FOUND: &str = "there is no such project";

fn refused(error: &Error) -> Response {
    let status = match error {
        Error::NotFound | Error::Join(JoinError::NotFound) => 404,
        Error::Rule(RuleError::Transition { .. } | RuleError::FinalOwner) => 409,
        Error::Denied(_)
        | Error::Rule(_)
        | Error::Join(JoinError::Blocked | JoinError::NotYours) => 403,
        Error::Invalid(_) | Error::Key | Error::Join(JoinError::Note) => 400,
        Error::Join(JoinError::Cooldown | JoinError::Queue) => 429,
        Error::Store(StoreError::Unavailable) => 503,
        Error::Join(_) | Error::Store(_) | Error::Lifecycle { .. } => 409,
    };
    failure(status, &error.to_string())
}

fn answer<T>(result: Result<T, Error>, body: impl FnOnce(T) -> Value) -> Response {
    match result {
        Ok(value) => reply(200, body(value)),
        Err(error) => refused(&error),
    }
}

fn record(value: &impl serde::Serialize) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// What anyone may know of a listed project.
fn listing(p: &Project) -> Value {
    json!({
        "id": p.id, "slug": p.slug, "name": p.name, "summary": p.summary,
        "visibility": p.visibility, "lifecycle": p.lifecycle, "join_policy": p.join_policy,
    })
}

fn body<T: DeserializeOwned>(request: &Request<'_>) -> Result<T, Response> {
    serde_json::from_slice(request.body)
        .map_err(|_| failure(400, "the request body is not what this call takes"))
}

fn parameter<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value)
}

#[derive(Deserialize)]
struct Create {
    slug: String,
    name: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    visibility: Visibility,
    #[serde(default)]
    join_policy: JoinPolicy,
}

#[derive(Deserialize)]
struct SetVisibility {
    visibility: Visibility,
}

#[derive(Deserialize)]
struct SetLifecycle {
    lifecycle: Lifecycle,
}

/// `user` here is the person being invited or offered, never the caller.
#[derive(Deserialize)]
struct Invite {
    user: String,
    role: Role,
}

#[derive(Deserialize)]
struct SetRole {
    role: Role,
}

#[derive(Deserialize)]
struct SetStatus {
    status: Status,
    #[serde(default)]
    block: bool,
}

#[derive(Deserialize)]
struct Join {
    #[serde(default)]
    note: String,
}

#[derive(Deserialize)]
struct Decide {
    revision: u64,
    /// The role to approve with; absent or null rejects.
    #[serde(default)]
    approve_as: Option<Role>,
}

#[derive(Deserialize)]
struct Offer {
    user: String,
    #[serde(default)]
    confirmed: bool,
}

/// Handles one request at time `now` (milliseconds since the Unix epoch).
#[must_use]
pub fn handle(db: &Database, now: u64, request: Request<'_>) -> Response {
    // Drawn before anything is done: without randomness there is no id that could not be guessed or collide.
    let drawn: Result<Vec<String>, StoreError> =
        (0..IDS_PER_CALL).map(|_| random_hex(16)).collect();
    let drawn = match drawn {
        Ok(drawn) => std::sync::Mutex::new(drawn),
        Err(error) => return refused(&error.into()),
    };
    let ids = move || {
        drawn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop()
            .unwrap_or_default()
    };
    let projects = Projects {
        store: db,
        ids: &ids,
    };
    let segments: Vec<&str> = request.path.split('/').filter(|s| !s.is_empty()).collect();
    // The public directory is the one call that needs no sign-in.
    if request.method == "GET" && segments == ["v1", "directory"] {
        let found = projects.directory(parameter(request.query, "after"), 50);
        return answer(
            found,
            |list| json!({ "projects": list.iter().map(listing).collect::<Vec<_>>() }),
        );
    }
    let user = match request.bearer.map(|token| db.user_of(token, now)) {
        Some(Ok(Some(user))) => user,
        Some(Err(error)) => return refused(&error.into()),
        None | Some(Ok(None)) => return failure(401, "sign in first"),
    };
    let correlation = match random_hex(8) {
        Ok(id) => id,
        Err(error) => return refused(&error.into()),
    };
    let call = Call {
        user,
        now,
        correlation,
    };
    match route(&projects, &call, &segments, &request) {
        Ok(response) | Err(response) => response,
    }
}

fn route(
    projects: &Projects<'_, Database>,
    call: &Call,
    segments: &[&str],
    request: &Request<'_>,
) -> Result<Response, Response> {
    let key = request.idempotency_key;
    Ok(match (request.method, segments) {
        ("GET", ["v1", "me"]) => reply(200, json!({ "user": call.user })),
        ("POST", ["v1", "projects"]) => {
            let new: Create = body(request)?;
            let new = NewProject {
                slug: new.slug,
                name: new.name,
                summary: new.summary,
                visibility: new.visibility,
                join_policy: new.join_policy,
            };
            answer(projects.create(call, new, key), |p| record(&p))
        }
        ("POST", ["v1", "join-requests", id, "decide"]) => {
            let decide: Decide = body(request)?;
            let done = projects.decide_join(call, id, decide.revision, decide.approve_as);
            answer(done, |r| record(&r))
        }
        ("POST", ["v1", "join-requests", id, "withdraw"]) => {
            answer(projects.withdraw_join(call, id), |r| record(&r))
        }
        (_, ["v1", "projects", id, rest @ ..]) => {
            project_route(projects, call, &ProjectId((*id).to_owned()), rest, request)?
        }
        _ => failure(404, "there is no such call"),
    })
}

fn project_route(
    projects: &Projects<'_, Database>,
    call: &Call,
    id: &ProjectId,
    rest: &[&str],
    request: &Request<'_>,
) -> Result<Response, Response> {
    Ok(match (request.method, rest) {
        ("GET", []) => {
            // Members get the whole record; others only the listing.
            let member = projects
                .store
                .member(id, &call.user)
                .map_err(|e| refused(&e.into()))?;
            let inside = member.is_some_and(|m| m.status == Status::Active);
            answer(projects.view(call, id), |p| {
                if inside { record(&p) } else { listing(&p) }
            })
        }
        ("PUT", ["visibility"]) => {
            let to: SetVisibility = body(request)?;
            answer(projects.set_visibility(call, id, to.visibility), |p| {
                record(&p)
            })
        }
        ("PUT", ["lifecycle"]) => {
            let to: SetLifecycle = body(request)?;
            answer(projects.set_lifecycle(call, id, to.lifecycle), |p| {
                record(&p)
            })
        }
        ("GET", ["members"]) => {
            let after = parameter(request.query, "after").map(UserId::from);
            answer(
                projects.members(call, id, after.as_ref()),
                |m| json!({ "members": m }),
            )
        }
        ("POST", ["invitations"]) => {
            let invite: Invite = body(request)?;
            if !valid_user(&invite.user) {
                return Err(failure(400, "that is not a user id"));
            }
            let made = projects.invite(call, id, &UserId(invite.user), invite.role);
            answer(made, |m| record(&m))
        }
        ("POST", ["invitations", "accept"]) => {
            answer(projects.accept_invite(call, id), |m| record(&m))
        }
        ("PUT", ["members", user, "role"]) => {
            let to: SetRole = body(request)?;
            let changed = projects.change_role(call, id, &UserId::from(*user), to.role);
            answer(changed, |m| record(&m))
        }
        ("PUT", ["members", user, "status"]) => {
            let to: SetStatus = body(request)?;
            let user = UserId::from(*user);
            let changed = projects.set_member_status(call, id, &user, to.status, to.block);
            answer(changed, |m| record(&m))
        }
        ("POST", ["join-requests"]) => {
            let join: Join = body(request)?;
            let made = projects.request_join(call, id, &join.note, request.idempotency_key);
            answer(made, |r| record(&r))
        }
        ("GET", ["join-requests"]) => answer(
            projects.waiting_requests(call, id),
            |r| json!({ "requests": r }),
        ),
        ("POST", ["owner", "offer"]) => {
            let offer: Offer = body(request)?;
            let user = UserId(offer.user);
            let done = projects.offer_ownership(call, id, &user, offer.confirmed);
            answer(done, |()| json!({}))
        }
        ("POST", ["owner", "accept"]) => {
            answer(projects.accept_ownership(call, id), |()| json!({}))
        }
        ("GET", ["audit"]) => answer(projects.audit_log(call, id), |e| json!({ "events": e })),
        _ => failure(404, NOT_FOUND),
    })
}

#[cfg(test)]
#[path = "api_tests.rs"]
mod tests;
