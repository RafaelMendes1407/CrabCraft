//! A real Axum router for the IDE's static endpoint tests.
use axum::{Router, extract::Path, routing::{get, post}};

async fn users(Path(id): Path<String>) -> String { format!("user:{id}") }
async fn save(Path(id): Path<String>) -> String { format!("saved:{id}") }
async fn health() -> &'static str { "healthy" }
async fn created() -> &'static str { "created" }

fn admin() -> Router { Router::new().route(r#"/health"#, get(health)) }

pub fn app() -> Router {
    let users = Router::new().route("/users/{id}", get(users).post(save));
    Router::new().nest("/api", users).nest("/admin", admin()).route("/create", post(created))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn serves_a_real_parameterized_endpoint() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app()).await.unwrap(); });
        let response = reqwest::Client::new().get(format!("http://{address}/api/users/42")).send().await.unwrap();
        assert_eq!(response.text().await.unwrap(), "user:42");
        server.abort();
    }
}
