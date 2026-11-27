//! The plan's acceptance scenarios (§16) as a client meets them, and hostile input.

use super::*;

struct Hub {
    db: Database,
}

impl Hub {
    fn new() -> Self {
        Self {
            db: Database::in_memory().expect("opens"),
        }
    }

    fn token(&self, user: &str) -> String {
        self.db
            .issue_token(&user.into(), 0, 1_000_000)
            .expect("issued")
    }

    fn send(&self, token: &str, method: &str, path: &str, body: &Value) -> Response {
        handle(
            &self.db,
            1000,
            Request {
                method,
                path,
                query: "",
                bearer: Some(token),
                idempotency_key: None,
                body: body.to_string().as_bytes(),
            },
        )
    }

    /// An active public project; returns its id.
    fn project(&self, owner: &str, slug: &str) -> String {
        let made = self.send(
            owner,
            "POST",
            "/v1/projects",
            &json!({ "slug": slug, "name": slug, "visibility": "public" }),
        );
        assert_eq!(made.status, 200, "{:?}", made.body);
        let id = made.body["id"].as_str().expect("id").to_owned();
        let path = format!("/v1/projects/{id}/lifecycle");
        let active = self.send(owner, "PUT", &path, &json!({ "lifecycle": "active" }));
        assert_eq!(active.status, 200, "{:?}", active.body);
        id
    }
}

#[test]
fn ask_approve_and_two_reviewers_racing() {
    let hub = Hub::new();
    let (owner, ann) = (hub.token("owner"), hub.token("ann"));
    let id = hub.project(&owner, "demo");
    let asked = hub.send(
        &ann,
        "POST",
        &format!("/v1/projects/{id}/join-requests"),
        &json!({ "note": "hello" }),
    );
    assert_eq!(asked.status, 200, "{:?}", asked.body);
    let request = asked.body["id"].as_str().expect("id").to_owned();
    let decide = format!("/v1/join-requests/{request}/decide");
    let own = hub.send(
        &ann,
        "POST",
        &decide,
        &json!({ "revision": 1, "approve_as": "admin" }),
    );
    assert_eq!(
        own.status, 403,
        "a requester cannot decide their own request"
    );
    let approve = json!({ "revision": 1, "approve_as": "contributor" });
    assert_eq!(hub.send(&owner, "POST", &decide, &approve).status, 200);
    let second = hub.send(&owner, "POST", &decide, &json!({ "revision": 1 }));
    assert_eq!(second.status, 409);
}

#[test]
fn a_changed_id_reads_nothing_and_a_removed_members_old_token_is_useless() {
    let hub = Hub::new();
    let (owner, dev, eve) = (hub.token("owner"), hub.token("dev"), hub.token("eve"));
    let made = hub.send(
        &eve,
        "POST",
        "/v1/projects",
        &json!({ "slug": "eves", "name": "e" }),
    );
    assert_eq!(made.status, 200);
    let secret = hub.send(
        &owner,
        "POST",
        "/v1/projects",
        &json!({ "slug": "secret", "name": "s" }),
    );
    let secret = secret.body["id"].as_str().expect("id").to_owned();
    // Private: the same answer as a project that does not exist.
    for tail in ["", "/members", "/audit", "/join-requests"] {
        let real = hub.send(
            &eve,
            "GET",
            &format!("/v1/projects/{secret}{tail}"),
            &json!({}),
        );
        let none = hub.send(&eve, "GET", &format!("/v1/projects/nope{tail}"), &json!({}));
        assert_eq!(real, none, "{tail}");
        assert_eq!(real.status, 404);
    }
    let id = hub.project(&owner, "demo");
    let invite = format!("/v1/projects/{id}/invitations");
    let body = json!({ "user": "dev", "role": "contributor" });
    assert_eq!(hub.send(&owner, "POST", &invite, &body).status, 200);
    let accept = format!("{invite}/accept");
    assert_eq!(hub.send(&dev, "POST", &accept, &json!({})).status, 200);
    let members = format!("/v1/projects/{id}/members");
    assert_eq!(hub.send(&dev, "GET", &members, &json!({})).status, 200);
    let status = format!("{members}/dev/status");
    let removed = json!({ "status": "removed" });
    assert_eq!(hub.send(&owner, "PUT", &status, &removed).status, 200);
    assert_eq!(hub.send(&dev, "GET", &members, &json!({})).status, 403);
}

#[test]
fn who_is_asking_comes_from_the_token_never_the_body() {
    let hub = Hub::new();
    let (owner, eve) = (hub.token("owner"), hub.token("eve"));
    let id = hub.project(&owner, "demo");
    let claim =
        json!({ "user": "owner", "role": "owner", "caller": "owner", "lifecycle": "archived" });
    let tried = hub.send(&eve, "PUT", &format!("/v1/projects/{id}/lifecycle"), &claim);
    assert_eq!(
        tried.status, 403,
        "claiming to be the owner changes nothing"
    );
    let public = hub.send(&eve, "GET", &format!("/v1/projects/{id}"), &json!({}));
    assert_eq!(public.status, 200);
    assert_eq!(
        public.body.get("policy"),
        None,
        "outsiders get the listing only"
    );
}

#[test]
fn no_token_or_a_wrong_one_gets_only_the_directory() {
    let hub = Hub::new();
    let owner = hub.token("owner");
    hub.project(&owner, "demo");
    let get = |path, bearer| {
        handle(
            &hub.db,
            1000,
            Request {
                method: "GET",
                path,
                bearer,
                ..Request::default()
            },
        )
    };
    let listed = get("/v1/directory", None);
    assert_eq!(listed.status, 200);
    assert_eq!(listed.body["projects"].as_array().map(Vec::len), Some(1));
    assert_eq!(get("/v1/me", None).status, 401);
    assert_eq!(get("/v1/me", Some("vivs_forged")).status, 401);
}

#[test]
fn hostile_bodies_and_paths_are_refused_without_trouble() {
    let hub = Hub::new();
    let owner = hub.token("owner");
    let id = hub.project(&owner, "demo");
    let raw = |method, path: &str, body: &[u8]| {
        handle(
            &hub.db,
            1000,
            Request {
                method,
                path,
                bearer: Some(&owner),
                body,
                ..Request::default()
            },
        )
        .status
    };
    let invite = format!("/v1/projects/{id}/invitations");
    let deep = "[".repeat(10_000);
    for body in [&b""[..], b"{", b"null", b"[]", b"\xff\xfe", deep.as_bytes()] {
        assert_eq!(raw("POST", "/v1/projects", body), 400);
        assert_eq!(raw("POST", &invite, body), 400);
    }
    let odd = br#"{"user":"../../etc","role":"viewer"}"#;
    assert_eq!(raw("POST", &invite, odd), 400);
    let unknown = br#"{"user":"a","role":"root"}"#;
    assert_eq!(raw("POST", &invite, unknown), 400);
    for path in [
        "/",
        "/v1",
        "/v1/projects/../..",
        "/v2/me",
        "/v1/join-requests/x/decide/y",
    ] {
        assert_eq!(raw("GET", path, b""), 404, "{path}");
    }
    assert_eq!(raw("DELETE", "/v1/me", b""), 404);
}
