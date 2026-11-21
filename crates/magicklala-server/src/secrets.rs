use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use magicklala_graph::{CredentialResolver, LlmGateway, NodeError};
use magicklala_nodes::{HttpLlmGateway, LlmGatewayConfig};
use magicklala_store::{CredentialId, CryptoVault, ProjectId, Repository};
use uuid::Uuid;

pub async fn project_secret(store: &dyn Repository, vault: &dyn CryptoVault, project: ProjectId, credential: CredentialId) -> Result<String, String> {
    let unavailable = || format!("credential {} is not available to project {}", credential.0, project.0);
    let owned = store.list_credentials(project).await.map_err(|_| unavailable())?.iter().any(|meta| meta.id == credential && meta.project_id == project);
    if !owned {
        return Err(unavailable());
    }
    let secret = store.reveal_credential(credential, vault).await.map_err(|_| format!("credential {} cannot be decrypted", credential.0))?;
    if secret.is_empty() { Err(format!("credential {} is empty", credential.0)) } else { Ok(secret) }
}

pub struct VaultCredentialResolver {
    store: Arc<dyn Repository>,
    vault: Arc<dyn CryptoVault>,
    project: ProjectId,
}

impl VaultCredentialResolver {
    pub fn new(store: Arc<dyn Repository>, vault: Arc<dyn CryptoVault>, project: ProjectId) -> Self {
        Self { store, vault, project }
    }

    pub fn project(&self) -> ProjectId {
        self.project
    }
}

#[async_trait]
impl CredentialResolver for VaultCredentialResolver {
    async fn resolve(&self, credential_id: &str) -> Result<String, NodeError> {
        let id = Uuid::parse_str(credential_id.trim()).map_err(|_| NodeError::InvalidConfig(format!("credential {credential_id} is not a credential id")))?;
        project_secret(self.store.as_ref(), self.vault.as_ref(), self.project, CredentialId(id)).await.map_err(NodeError::InvalidConfig)
    }
}

#[derive(Clone)]
pub enum LlmSource {
    Http(LlmGatewayConfig),
    Fixed(Arc<dyn LlmGateway>),
}

pub struct ProjectLlms {
    source: LlmSource,
    shared: Arc<dyn LlmGateway>,
    store: Arc<dyn Repository>,
    vault: Arc<dyn CryptoVault>,
    projects: Mutex<HashMap<ProjectId, Arc<dyn LlmGateway>>>,
}

impl ProjectLlms {
    pub fn new(source: LlmSource, store: Arc<dyn Repository>, vault: Arc<dyn CryptoVault>) -> Self {
        let shared: Arc<dyn LlmGateway> = match &source {
            LlmSource::Http(config) => Arc::new(HttpLlmGateway::new(config.clone())),
            LlmSource::Fixed(llm) => Arc::clone(llm),
        };
        Self { source, shared, store, vault, projects: Mutex::new(HashMap::new()) }
    }

    pub fn shared(&self) -> Arc<dyn LlmGateway> {
        Arc::clone(&self.shared)
    }

    pub fn for_project(&self, project: ProjectId) -> Arc<dyn LlmGateway> {
        let LlmSource::Http(config) = &self.source else { return self.shared() };
        let mut projects = self.projects.lock().unwrap_or_else(PoisonError::into_inner);
        let gateway = projects.entry(project).or_insert_with(|| {
            let resolver = VaultCredentialResolver::new(Arc::clone(&self.store), Arc::clone(&self.vault), project);
            Arc::new(HttpLlmGateway::new(config.clone()).with_credential_resolver(Arc::new(resolver)))
        });
        Arc::clone(gateway)
    }
}
