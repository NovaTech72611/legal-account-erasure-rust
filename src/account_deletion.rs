use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::infrai_client::{AccessRevoker, InfraiError};

#[derive(Debug, Clone, Deserialize)]
pub struct DeletionRequest {
    pub account_id: String,
    pub infrai_key_id: String,
    pub matter_intake_ids: Vec<String>,
    pub signed_delivery_ids: Vec<String>,
    pub deadline_follow_up_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DeletionReceipt {
    pub account_id: String,
    pub sessions_revoked: usize,
    pub credential_revoked: bool,
    pub matter_intakes_deleted: usize,
    pub signed_deliveries_deleted: usize,
    pub deadline_follow_ups_deleted: usize,
    pub state: &'static str,
}

#[derive(Debug, Error)]
pub enum DeletionError {
    #[error("account_id and infrai_key_id are required")]
    InvalidRequest,
    #[error(transparent)]
    Infrai(#[from] InfraiError),
    #[error("legal records could not be deleted: {0}")]
    Store(String),
}

#[async_trait]
pub trait LegalRecordStore: Send + Sync {
    async fn delete_account_records(&self, request: &DeletionRequest) -> Result<(), String>;
}

pub struct DeletionCoordinator<R, S> {
    revoker: R,
    store: S,
}

impl<R, S> DeletionCoordinator<R, S>
where
    R: AccessRevoker,
    S: LegalRecordStore,
{
    pub fn new(revoker: R, store: S) -> Self {
        Self { revoker, store }
    }

    pub async fn delete_account(
        &self,
        request: DeletionRequest,
    ) -> Result<DeletionReceipt, DeletionError> {
        if request.account_id.trim().is_empty() || request.infrai_key_id.trim().is_empty() {
            return Err(DeletionError::InvalidRequest);
        }

        let sessions = self.revoker.list_session_ids(&request.account_id).await?;
        for session_id in &sessions {
            self.revoker.revoke_session(session_id).await?;
        }
        self.revoker
            .revoke_account_key(&request.infrai_key_id)
            .await?;
        self.store
            .delete_account_records(&request)
            .await
            .map_err(DeletionError::Store)?;

        Ok(DeletionReceipt {
            account_id: request.account_id,
            sessions_revoked: sessions.len(),
            credential_revoked: true,
            matter_intakes_deleted: request.matter_intake_ids.len(),
            signed_deliveries_deleted: request.signed_delivery_ids.len(),
            deadline_follow_ups_deleted: request.deadline_follow_up_ids.len(),
            state: "deleted",
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::infrai_client::InfraiRejection;

    struct FailingRevoker;

    #[async_trait]
    impl AccessRevoker for FailingRevoker {
        async fn list_session_ids(&self, _: &str) -> Result<Vec<String>, InfraiError> {
            Ok(vec!["session-a".to_owned(), "session-b".to_owned()])
        }

        async fn revoke_session(&self, session_id: &str) -> Result<(), InfraiError> {
            if session_id == "session-b" {
                return Err(InfraiError::Rejected(InfraiRejection {
                    code: String::new(),
                    message: "session remains active".to_owned(),
                    status: 409,
                }));
            }
            Ok(())
        }

        async fn revoke_account_key(&self, _: &str) -> Result<(), InfraiError> {
            Ok(())
        }
    }

    #[derive(Clone)]
    struct RecordingStore(Arc<Mutex<bool>>);

    #[async_trait]
    impl LegalRecordStore for RecordingStore {
        async fn delete_account_records(&self, _: &DeletionRequest) -> Result<(), String> {
            *self.0.lock().expect("recording lock") = true;
            Ok(())
        }
    }

    #[tokio::test]
    async fn keeps_legal_records_when_a_session_cannot_be_revoked() {
        let deleted = Arc::new(Mutex::new(false));
        let coordinator = DeletionCoordinator::new(FailingRevoker, RecordingStore(deleted.clone()));

        let result = coordinator
            .delete_account(DeletionRequest {
                account_id: "acct-42".to_owned(),
                infrai_key_id: "key-42".to_owned(),
                matter_intake_ids: vec!["matter-7".to_owned()],
                signed_delivery_ids: vec!["delivery-3".to_owned()],
                deadline_follow_up_ids: vec!["deadline-9".to_owned()],
            })
            .await;

        assert!(matches!(result, Err(DeletionError::Infrai(_))));
        assert!(!*deleted.lock().expect("recording lock"));
    }
}
