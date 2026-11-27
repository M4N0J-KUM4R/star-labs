//! The front door's contract: a request the port cannot answer goes to
//! Canvas, and Canvas' answer comes back unchanged.
//!
//! The test runs a mock Canvas — an axum server that echoes what it
//! received — and puts the front door in front of it.

use std::net::SocketAddr;

use axum::{
    extract::Request,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Extension, Router,
};
use serde_json::{json, Value};
use star_proxy::{CanvasOrigin, ORIGIN_ENV};
use tokio::net::TcpListener;

/// The mock Canvas: it answers with the request it saw.
async fn echo(req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 1024 * 1024)
        .await
        .unwrap_or_default();
    let seen = json!({
        "method": parts.method.as_str(),
        "path": parts.uri.path(),
        "query": parts.uri.query(),
        "body": String::from_utf8_lossy(&bytes),
        "authorization": parts.headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()),
        "content_type": parts.headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()),
    });
    let mut response = (StatusCode::OK, seen.to_string()).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json; charset=utf-8"),
    );
    response.headers_mut().append(
        header::SET_COOKIE,
        header::HeaderValue::from_static("canvas_session=one; Path=/"),
    );
    response.headers_mut().append(
        header::SET_COOKIE,
        header::HeaderValue::from_static("canvas_session=two; Path=/"),
    );
    response
}

/// Start the mock Canvas and return its address.
async fn start_canvas() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = Router::new().route(
        "/api/v1/canvas/echo",
        get(echo).post(echo).put(echo).delete(echo),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

/// The port's router: it owns one route and delegates the rest.
fn front_door(origin: Option<CanvasOrigin>) -> Router {
    Router::new()
        .route("/api/v1/owned", get(|| async { "owned" }))
        .fallback(star_proxy::fallback)
        .method_not_allowed_fallback(star_proxy::fallback)
        .layer(Extension(origin.map(std::sync::Arc::new)))
}

/// The port's router as the binary builds it: the API router is nested under
/// `/api/v1`, so axum strips the prefix before the fallback runs.
fn nested_front_door(origin: Option<CanvasOrigin>) -> Router {
    Router::new()
        .nest(
            "/api/v1",
            Router::new()
                .route("/owned", get(|| async { "owned" }))
                .fallback(star_proxy::fallback)
                .method_not_allowed_fallback(star_proxy::fallback),
        )
        .layer(Extension(origin.map(std::sync::Arc::new)))
}

/// Serve the port and return its address.
async fn start_port(app: Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

#[tokio::test]
async fn a_request_the_port_cannot_serve_goes_to_canvas() {
    let canvas = start_canvas().await;
    let origin = CanvasOrigin::new(&format!("http://{canvas}")).unwrap();
    let port = start_port(front_door(Some(origin))).await;

    let client = reqwest::Client::new();
    let response = client
        .post(format!("http://{port}/api/v1/canvas/echo?a=1"))
        .header(header::AUTHORIZATION, "Bearer token-abc")
        .header(header::CONTENT_TYPE, "application/json")
        .body(r#"{"hello":"world"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let seen: Value = response.json().await.unwrap();
    assert_eq!(seen["method"], "POST");
    assert_eq!(seen["path"], "/api/v1/canvas/echo");
    assert_eq!(seen["query"].as_str(), Some("a=1"));
    assert_eq!(seen["body"], r#"{"hello":"world"}"#);
    assert_eq!(seen["authorization"].as_str(), Some("Bearer token-abc"));
    assert_eq!(seen["content_type"].as_str(), Some("application/json"));
}

#[tokio::test]
async fn canvases_answer_comes_back_unchanged() {
    let canvas = start_canvas().await;
    let origin = CanvasOrigin::new(&format!("http://{canvas}")).unwrap();
    let port = start_port(front_door(Some(origin))).await;

    let client = reqwest::Client::new();
    let response = client
        .get(format!("http://{port}/api/v1/canvas/echo"))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    assert_eq!(
        response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .count(),
        2,
        "every Set-Cookie header must survive the delegation"
    );
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/json; charset=utf-8")
    );
}

#[tokio::test]
async fn a_route_the_port_owns_is_answered_by_the_port() {
    let canvas = start_canvas().await;
    let origin = CanvasOrigin::new(&format!("http://{canvas}")).unwrap();
    let port = start_port(front_door(Some(origin))).await;

    let client = reqwest::Client::new();
    let response = client
        .get(format!("http://{port}/api/v1/owned"))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    assert_eq!(response.text().await.unwrap(), "owned");
}

#[tokio::test]
async fn without_a_canvas_the_front_door_answers_an_empty_404() {
    let port = start_port(front_door(None)).await;

    let client = reqwest::Client::new();
    let response = client
        .get(format!("http://{port}/api/v1/nothing/here"))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 404);
    assert_eq!(response.text().await.unwrap(), "");
}

#[tokio::test]
async fn a_nested_router_delegates_the_path_the_request_came_in_with() {
    let canvas = start_canvas().await;
    let origin = CanvasOrigin::new(&format!("http://{canvas}")).unwrap();
    let port = start_port(nested_front_door(Some(origin))).await;

    let client = reqwest::Client::new();
    let response = client
        .get(format!("http://{port}/api/v1/canvas/echo?a=1"))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let seen: Value = response.json().await.unwrap();
    // axum's `nest` stripped the prefix before the fallback ran; the
    // delegation must put it back, because Canvas serves `/api/v1`.
    assert_eq!(seen["path"], "/api/v1/canvas/echo");
    assert_eq!(seen["query"].as_str(), Some("a=1"));
}

#[tokio::test]
async fn the_origin_is_read_from_the_environment() {
    std::env::set_var(ORIGIN_ENV, "https://canvas.example.edu");
    let origin = CanvasOrigin::from_env().unwrap().unwrap();
    assert_eq!(origin.base(), "https://canvas.example.edu");
    std::env::remove_var(ORIGIN_ENV);
    assert!(CanvasOrigin::from_env().unwrap().is_none());
}

#[tokio::test]
async fn a_method_the_port_does_not_serve_goes_to_canvas() {
    let canvas = start_canvas().await;
    let origin = CanvasOrigin::new(&format!("http://{canvas}")).unwrap();
    let port = start_port(front_door(Some(origin))).await;

    let client = reqwest::Client::new();
    let response = client
        .post(format!("http://{port}/api/v1/owned"))
        .send()
        .await
        .unwrap();

    // Canvas answers a route it does not define with 404, not 405.
    assert_ne!(response.status(), 405);
}
