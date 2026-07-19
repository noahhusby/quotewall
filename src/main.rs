mod printer;

use axum::Router;
use axum::routing::get;

struct PrintRequest {
    message: String,
    subject: String,
}

#[tokio::main]
async fn main() {
    env_logger::init();
    let app = Router::new().route("/", get(|| async { "Hello, World!" }));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}