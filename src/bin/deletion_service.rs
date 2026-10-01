use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use legal_account_erasure::{
    account_deletion::{DeletionCoordinator, DeletionError, DeletionRequest, LegalRecordStore},
    infrai_client::{InfraiClient, InfraiError},
};
use serde_json::json;

#[derive(Clone)]
struct Repository;

#[async_trait]
impl LegalRecordStore for Repository {
    async fn delete_account_records(&self, request: &DeletionRequest) -> Result<(), String> {
        println!(
            "deleted account={} matter_intakes={} signed_deliveries={} deadline_follow_ups={}",
            request.account_id,
            request.matter_intake_ids.len(),
            request.signed_delivery_ids.len(),
            request.deadline_follow_up_ids.len()
        );
        Ok(())
    }
}

type Coordinator = DeletionCoordinator<InfraiClient, Repository>;

async fn delete_account(
    State(coordinator): State<Arc<Coordinator>>,
    Json(request): Json<DeletionRequest>,
) -> Result<impl IntoResponse, ServiceError> {
    let receipt = coordinator.delete_account(request).await?;
    Ok((StatusCode::OK, Json(receipt)))
}

struct ServiceError(DeletionError);

impl From<DeletionError> for ServiceError {
    fn from(value: DeletionError) -> Self {
        Self(value)
    }
}

impl IntoResponse for ServiceError {
    fn into_response(self) -> Response {
        let (status, message) = match self.0 {
            DeletionError::InvalidRequest => (StatusCode::BAD_REQUEST, "invalid deletion request"),
            DeletionError::Infrai(InfraiError::Rejected(rejection)) => {
                let status = StatusCode::from_u16(rejection.status)
                    .unwrap_or(StatusCode::UNPROCESSABLE_ENTITY);
                return (
                    status,
                    Json(json!({
                        "error": rejection.code,
                        "message": rejection.message
                    })),
                )
                    .into_response();
            }
            DeletionError::Infrai(InfraiError::Server(_))
            | DeletionError::Infrai(InfraiError::Transport(_)) => {
                (StatusCode::BAD_GATEWAY, "upstream request failed")
            }
            DeletionError::Infrai(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "service configuration error",
            ),
            DeletionError::Store(_) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "record deletion failed")
            }
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let infrai = InfraiClient::from_env()?;
    let coordinator = Arc::new(DeletionCoordinator::new(infrai, Repository));
    let app = Router::new()
        .route("/accounts/delete", post(delete_account))
        .with_state(coordinator);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await?;
    println!("deletion_service listening on http://127.0.0.1:3000");
    axum::serve(listener, app).await?;
    Ok(())
}
