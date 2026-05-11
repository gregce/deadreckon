use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use deadreckon_providers::{
    ProviderConfigFile, ProviderEntry, ProviderKind, ProviderRequest, ProviderRouter,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::net::TcpListener;

#[derive(Clone)]
struct MockState {
    fixtures: Arc<Mutex<Vec<FixtureResponse>>>,
    journal: Arc<Mutex<Vec<Value>>>,
}

#[derive(Debug, Clone, Deserialize)]
struct FixtureResponse {
    id: Option<String>,
    model: Option<String>,
    content: Option<String>,
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    status: Option<u16>,
    body: Option<Value>,
    delay_ms: Option<u64>,
}

#[tokio::test]
async fn mock_provider_records_three_turns() {
    let server = MockServer::start(include_str!("fixtures/mock-script-three-turn.json")).await;
    let router = ProviderRouter::from_config(
        ProviderConfigFile {
            default_provider: None,
            fallback: Some(vec!["mock".to_string()]),
            providers: [(
                "mock".to_string(),
                ProviderEntry {
                    kind: Some(ProviderKind::OpenAiCompatible),
                    api_key: Some("test-key".to_string()),
                    api_key_env: None,
                    base_url: Some(server.base_url()),
                    model: Some("mock-agent".to_string()),
                    input_cost_per_million: Some(1.0),
                    output_cost_per_million: Some(2.0),
                    binary: None,
                    extra_args: Vec::new(),
                },
            )]
            .into_iter()
            .collect(),
        },
        None,
    )
    .expect("router");

    let mut contents = Vec::new();
    for turn in 1..=3 {
        let response = router
            .complete(&ProviderRequest {
                prompt: format!("turn {turn}"),
                max_output_tokens: 1024,
                cwd: None,
                output_path: None,
            })
            .await
            .expect("completion");
        contents.push(response.content);
    }

    assert!(contents[0].contains("\"action\":\"bash\""));
    assert!(contents[1].contains("\"action\":\"write_file\""));
    assert!(contents[2].contains("\"action\":\"done\""));
    let journal = server.journal();
    assert_eq!(journal.len(), 3);
    assert_eq!(journal[0]["model"], "mock-agent");
}

#[tokio::test]
async fn mock_provider_supports_error_fixture() {
    let server = MockServer::start(include_str!("fixtures/mock-script-error.json")).await;
    let router = ProviderRouter::from_config(
        ProviderConfigFile {
            default_provider: None,
            fallback: Some(vec!["mock".to_string()]),
            providers: [(
                "mock".to_string(),
                ProviderEntry {
                    kind: Some(ProviderKind::OpenAiCompatible),
                    api_key: Some("test-key".to_string()),
                    api_key_env: None,
                    base_url: Some(server.base_url()),
                    model: Some("mock-agent".to_string()),
                    input_cost_per_million: Some(0.0),
                    output_cost_per_million: Some(0.0),
                    binary: None,
                    extra_args: Vec::new(),
                },
            )]
            .into_iter()
            .collect(),
        },
        None,
    )
    .expect("router");

    let err = router
        .complete(&ProviderRequest {
            prompt: "fail".to_string(),
            max_output_tokens: 16,
            cwd: None,
            output_path: None,
        })
        .await
        .expect_err("fixture error");
    assert!(err.to_string().contains("HTTP 503"));
    assert_eq!(server.journal().len(), 1);
}

struct MockServer {
    addr: SocketAddr,
    state: MockState,
}

impl MockServer {
    async fn start(script: &str) -> Self {
        let fixtures = serde_json::from_str::<Vec<FixtureResponse>>(script).expect("fixtures");
        let state = MockState {
            fixtures: Arc::new(Mutex::new(fixtures)),
            journal: Arc::new(Mutex::new(Vec::new())),
        };
        let app = Router::new()
            .route("/chat/completions", post(chat_completions))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve mock provider");
        });
        Self { addr, state }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn journal(&self) -> Vec<Value> {
        self.state.journal.lock().expect("journal").clone()
    }
}

async fn chat_completions(
    State(state): State<MockState>,
    Json(request): Json<Value>,
) -> impl IntoResponse {
    state.journal.lock().expect("journal").push(request);
    let fixture = {
        let mut fixtures = state.fixtures.lock().expect("fixtures");
        if fixtures.is_empty() {
            None
        } else {
            Some(fixtures.remove(0))
        }
    };
    let Some(fixture) = fixture else {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"error": {"message": "no fixture response left"}})),
        );
    };
    if let Some(delay_ms) = fixture.delay_ms {
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
    }
    if let Some(status) = fixture.status {
        let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        return (
            status,
            Json(
                fixture
                    .body
                    .unwrap_or_else(|| json!({"error": {"message": "fixture error"}})),
            ),
        );
    }
    (
        StatusCode::OK,
        Json(json!({
            "id": fixture.id.unwrap_or_else(|| "mock".to_string()),
            "object": "chat.completion",
            "model": fixture.model.unwrap_or_else(|| "mock-agent".to_string()),
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": fixture.content.unwrap_or_default()
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": fixture.prompt_tokens.unwrap_or(0),
                "completion_tokens": fixture.completion_tokens.unwrap_or(0),
                "total_tokens": fixture.prompt_tokens.unwrap_or(0) + fixture.completion_tokens.unwrap_or(0)
            }
        })),
    )
}
