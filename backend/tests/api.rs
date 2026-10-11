use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use mymail_server::{api, db::Db};
use serde_json::{Value, json};
use tower::ServiceExt;

struct TestApp {
    router: Router,
}

impl TestApp {
    fn new() -> Self {
        TestApp {
            router: api::router(Db::open_in_memory().unwrap()),
        }
    }

    async fn call(&self, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(uri);
        let body = match body {
            Some(v) => {
                req = req.header("content-type", "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let res = self
            .router
            .clone()
            .oneshot(req.body(body).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| panic!("non-JSON body: {}", String::from_utf8_lossy(&bytes)))
        };
        (status, value)
    }

    async fn get(&self, uri: &str) -> (StatusCode, Value) {
        self.call(Method::GET, uri, None).await
    }

    async fn post(&self, uri: &str, body: Value) -> (StatusCode, Value) {
        self.call(Method::POST, uri, Some(body)).await
    }

    async fn account(&self, name: &str, email: &str) -> i64 {
        let (status, v) = self
            .post("/api/accounts", json!({"name": name, "email": email}))
            .await;
        assert_eq!(status, StatusCode::CREATED, "{v}");
        v["id"].as_i64().unwrap()
    }
}

#[tokio::test]
async fn health() {
    let app = TestApp::new();
    let (status, v) = app.get("/api/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v, json!({"status": "ok"}));
}

#[tokio::test]
async fn accounts_crud_and_errors() {
    let app = TestApp::new();
    let id = app.account("Me", "Me@Local.Test").await;

    let (status, v) = app.get("/api/accounts").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v[0]["email"], "me@local.test");
    assert!(v[0]["created_at"].is_i64());

    let (status, v) = app
        .post(
            "/api/accounts",
            json!({"name": "Dup", "email": "me@local.test"}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(v["error"].as_str().unwrap().contains("already exists"));

    let (status, v) = app
        .post("/api/accounts", json!({"name": "X", "email": "bad"}))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(v["error"].is_string());

    // Malformed JSON and missing fields also use the error envelope.
    let (status, v) = app.post("/api/accounts", json!({"name": "X"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(v["error"].is_string());

    let (status, v) = app
        .call(Method::DELETE, &format!("/api/accounts/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(v, Value::Null);

    let (status, v) = app
        .call(Method::DELETE, &format!("/api/accounts/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(v["error"], "account not found");

    let (status, v) = app.get("/api/accounts/abc/folders").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(v["error"].is_string());
}

#[tokio::test]
async fn send_list_and_folder_counts() {
    let app = TestApp::new();
    let a = app.account("A", "a@t.local").await;
    let b = app.account("B", "b@t.local").await;

    let (status, v) = app
        .post(
            "/api/send",
            json!({"account_id": a, "to": "B <b@t.local>, ext@example.com", "subject": "Hi", "body": "Hello there"}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{v}");
    assert_eq!(v["delivered_local"], 1);
    assert_eq!(v["undelivered"], json!(["ext@example.com"]));
    assert_eq!(v["message"]["folder"], "sent");
    assert_eq!(v["message"]["to_addrs"], "b@t.local, ext@example.com");
    assert_eq!(v["message"]["in_reply_to"], Value::Null);

    let (status, inbox) = app.get(&format!("/api/messages?account_id={b}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(inbox.as_array().unwrap().len(), 1);
    let m = &inbox[0];
    assert_eq!(m["subject"], "Hi");
    assert_eq!(m["snippet"], "Hello there");
    assert_eq!(m["from_addr"], "a@t.local");
    assert_eq!(m["is_read"], false);

    let (_, counts) = app.get(&format!("/api/accounts/{b}/folders")).await;
    let names: Vec<&str> = counts
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["folder"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["inbox", "starred", "sent", "drafts", "archive", "trash"]
    );
    assert_eq!(
        counts[0],
        json!({"folder": "inbox", "total": 1, "unread": 1})
    );

    let (status, v) = app
        .post(&format!("/api/accounts/{b}/folders/inbox/read"), json!({}))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v, json!({"updated": 1}));
    let (_, counts) = app.get(&format!("/api/accounts/{b}/folders")).await;
    assert_eq!(counts[0]["unread"], 0);

    let (status, _) = app
        .post(&format!("/api/accounts/{b}/folders/nope/read"), json!({}))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = app
        .post("/api/send", json!({"account_id": a, "to": ""}))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = app
        .post("/api/send", json!({"account_id": 999, "to": "b@t.local"}))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn list_validates_query() {
    let app = TestApp::new();
    let a = app.account("A", "a@t.local").await;
    assert_eq!(app.get("/api/messages").await.0, StatusCode::BAD_REQUEST);
    assert_eq!(
        app.get("/api/messages?account_id=999").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.get(&format!("/api/messages?account_id={a}&folder=bogus"))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let (status, v) = app
        .get(&format!("/api/messages?account_id={a}&folder=all"))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v, json!([]));
}

#[tokio::test]
async fn patch_and_delete_messages() {
    let app = TestApp::new();
    let a = app.account("A", "a@t.local").await;
    let b = app.account("B", "b@t.local").await;
    app.post(
        "/api/send",
        json!({"account_id": a, "to": "b@t.local", "subject": "S", "body": "x"}),
    )
    .await;
    let (_, inbox) = app.get(&format!("/api/messages?account_id={b}")).await;
    let id = inbox[0]["id"].as_i64().unwrap();

    let (status, m) = app.get(&format!("/api/messages/{id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(m["body"], "x");
    assert_eq!(m["cc_addrs"], "");

    let (status, m) = app
        .call(
            Method::PATCH,
            &format!("/api/messages/{id}"),
            Some(json!({"is_read": true, "is_starred": true})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (m["is_read"].as_bool(), m["is_starred"].as_bool()),
        (Some(true), Some(true))
    );

    let (_, starred) = app
        .get(&format!("/api/messages?account_id={b}&folder=starred"))
        .await;
    assert_eq!(starred.as_array().unwrap().len(), 1);

    let (status, m) = app
        .call(
            Method::PATCH,
            &format!("/api/messages/{id}"),
            Some(json!({"folder": "archive"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(m["folder"], "archive");
    let (status, _) = app
        .call(
            Method::PATCH,
            &format!("/api/messages/{id}"),
            Some(json!({"folder": "starred"})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, v) = app
        .call(Method::DELETE, &format!("/api/messages/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v, json!({"permanent": false}));
    let (_, trash) = app
        .get(&format!("/api/messages?account_id={b}&folder=trash"))
        .await;
    assert_eq!(trash[0]["id"], id);

    let (_, v) = app
        .call(Method::DELETE, &format!("/api/messages/{id}"), None)
        .await;
    assert_eq!(v, json!({"permanent": true}));
    assert_eq!(
        app.get(&format!("/api/messages/{id}")).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.call(
            Method::PATCH,
            &format!("/api/messages/{id}"),
            Some(json!({}))
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn empty_trash() {
    let app = TestApp::new();
    let a = app.account("A", "a@t.local").await;
    for s in ["1", "2"] {
        app.post(
            "/api/send",
            json!({"account_id": a, "to": "a@t.local", "subject": s}),
        )
        .await;
    }
    let (_, inbox) = app.get(&format!("/api/messages?account_id={a}")).await;
    for m in inbox.as_array().unwrap() {
        app.call(Method::DELETE, &format!("/api/messages/{}", m["id"]), None)
            .await;
    }
    let (status, v) = app
        .post(&format!("/api/accounts/{a}/trash/empty"), json!({}))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v, json!({"deleted": 2}));
}

#[tokio::test]
async fn drafts_lifecycle() {
    let app = TestApp::new();
    let a = app.account("A", "a@t.local").await;
    let b = app.account("B", "b@t.local").await;

    let (status, d) = app
        .post(
            "/api/drafts",
            json!({"account_id": a, "to": "b@t.lo", "subject": "Plan", "body": "v1"}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(d["folder"], "drafts");
    let draft_id = d["id"].as_i64().unwrap();

    let (status, d) = app
        .call(
            Method::PUT,
            &format!("/api/drafts/{draft_id}"),
            Some(json!({"account_id": a, "to": "b@t.local", "subject": "Plan", "body": "v2"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(d["body"], "v2");
    assert_eq!(d["to_addrs"], "b@t.local");

    let (status, _) = app
        .call(
            Method::PUT,
            "/api/drafts/999",
            Some(json!({"account_id": a})),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, v) = app
        .post(
            "/api/send",
            json!({"account_id": a, "to": "b@t.local", "subject": "Plan", "body": "v2", "draft_id": draft_id}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{v}");
    let (_, drafts) = app
        .get(&format!("/api/messages?account_id={a}&folder=drafts"))
        .await;
    assert_eq!(drafts, json!([]));
    let (_, inbox) = app.get(&format!("/api/messages?account_id={b}")).await;
    assert_eq!(inbox[0]["snippet"], "v2");
}

#[tokio::test]
async fn search() {
    let app = TestApp::new();
    let a = app.account("A", "a@t.local").await;
    app.post("/api/send", json!({"account_id": a, "to": "a@t.local", "subject": "Quarterly report", "body": "numbers"})).await;
    app.post(
        "/api/send",
        json!({"account_id": a, "to": "a@t.local", "subject": "Lunch", "body": "ramen"}),
    )
    .await;

    let (status, v) = app
        .get(&format!("/api/messages?account_id={a}&q=quart"))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v.as_array().unwrap().len(), 1);
    assert_eq!(v[0]["subject"], "Quarterly report");

    let (_, v) = app
        .get(&format!("/api/messages?account_id={a}&folder=all&q=ramen"))
        .await;
    assert_eq!(v.as_array().unwrap().len(), 2, "inbox + sent copies");

    let (status, v) = app
        .get(&format!("/api/messages?account_id={a}&q=%22%28AND"))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v, json!([]));
}
