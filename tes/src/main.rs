use std::{
    sync::Arc,
    sync::atomic::{AtomicU64, Ordering},
};
use tokio::fs;
use axum::{
    extract::State,
    routing::get,
    Router
};

type Counter = Arc<AtomicU64>;

fn fib(n: u64) -> u64 {
    match n {
        0 => 0,
        1 => 1,
        _ => fib(n - 1) + fib(n - 2),
    }
}

async fn one() -> String {
    let result = fib(40);
    let mut val = fs::read_to_string("data.txt").await.unwrap();
    val = format!("val: {}, fib: {}", val, result.to_string());
    val
}

async fn four(State(counter): State<Counter>) -> String {
    let req_num = counter.fetch_add(1, Ordering::Relaxed) + 1;
    format!("hello, request number: {}", req_num)
}

#[tokio::main]
async fn main() {
    if !std::path::Path::new("data.txt").exists() {
        fs::write("data.txt", "hello").await.unwrap();
    }

    let counter = Arc::new(AtomicU64::new(0));
    let counter_ref = counter.clone();

    let app = Router::new()
        .route("/1", get(one))
        .route("/4", get(four))
        .with_state(counter_ref);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();
    println!("backend is listening on port 8080");
    axum::serve(listener, app).await.unwrap();
}