use async_trait::async_trait;
use wanaku_types::credentials::{
    EnvResolver, NormalizedOrigin, ResolutionContext, ResolvedSecret, ResolverError, SecretRef,
    SecretResolver,
};

#[derive(Debug)]
pub struct HttpsEnvResolver {
    inner: EnvResolver,
}

impl HttpsEnvResolver {
    pub const fn new() -> Self {
        Self {
            inner: EnvResolver::new(),
        }
    }
}

#[async_trait]
impl SecretResolver for HttpsEnvResolver {
    fn scheme(&self) -> &str {
        "env"
    }

    async fn resolve(
        &self,
        secret_ref: &SecretRef,
        context: &ResolutionContext,
    ) -> Result<ResolvedSecret, ResolverError> {
        // Brokerage supplies this origin only after matching the final upstream address.
        let origin = context
            .upstream_origin
            .as_deref()
            .and_then(|address| NormalizedOrigin::from_address(address).ok())
            .ok_or(ResolverError::InvalidReference)?;
        if !origin.as_str().starts_with("https://") {
            return Err(ResolverError::InvalidReference);
        }
        self.inner.resolve(secret_ref, context).await
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::sync::Arc;

    use anyhow::{Result, ensure};
    use wanaku_infra::credentials::CredentialBroker;
    use wanaku_types::credentials::{
        BindingRestrictions, CacheRules, CredentialBinding, CredentialPurpose, InjectionMechanism,
        NormalizedOrigin, ResolutionContext, ResolverError, ResolverRegistry, SecretRef,
        SecretResolver, UseScope,
    };

    use super::HttpsEnvResolver;

    fn run_isolated(case: &str) -> Result<bool> {
        if std::env::var("RHAF_CORE_CREDENTIAL_CASE").as_deref() == Ok(case) {
            return Ok(false);
        }
        let status = Command::new(std::env::current_exe()?)
            .args(["--exact", case, "--nocapture"])
            .env("RHAF_CORE_CREDENTIAL_CASE", case)
            .env("RHAF_CORE_CREDENTIAL_FIXTURE", "synthetic-test-secret")
            .env_remove("PATH")
            .status()?;
        ensure!(status.success(), "isolated credential test failed: {case}");
        Ok(true)
    }

    #[tokio::test]
    async fn rejects_insecure_or_missing_origin_before_secret_resolution() -> Result<()> {
        if run_isolated(
            "credentials::tests::rejects_insecure_or_missing_origin_before_secret_resolution",
        )? {
            return Ok(());
        }
        let resolver = HttpsEnvResolver::new();
        let secret_ref = SecretRef::parse("env:RHAF_CORE_CREDENTIAL_FIXTURE")?;
        for origin in [None, Some("http://localhost:8081"), Some("invalid")] {
            let context = ResolutionContext {
                upstream_origin: origin.map(str::to_owned),
                ..ResolutionContext::default()
            };
            assert!(matches!(
                resolver.resolve(&secret_ref, &context).await,
                Err(ResolverError::InvalidReference)
            ));
        }
        let context = ResolutionContext {
            upstream_origin: Some("https://example.test:443".to_owned()),
            ..ResolutionContext::default()
        };
        assert!(resolver.resolve(&secret_ref, &context).await.is_ok());
        Ok(())
    }

    fn cached_binding() -> Result<CredentialBinding> {
        Ok(CredentialBinding {
            id: "test-binding".to_owned(),
            forward_id: "test-forward".to_owned(),
            origin: NormalizedOrigin::from_address("https://example.test")?,
            mechanism: InjectionMechanism::Bearer,
            secret_refs: vec![SecretRef::parse("env:RHAF_CORE_CREDENTIAL_FIXTURE")?],
            allowed_purposes: vec![CredentialPurpose::Discovery, CredentialPurpose::Invocation],
            restrictions: BindingRestrictions::default(),
            cache: CacheRules {
                max_ttl_seconds: Some(60),
            },
            revision: 1,
        })
    }

    async fn populate_https_cache(
        broker: &CredentialBroker,
        binding: &CredentialBinding,
    ) -> Result<()> {
        broker
            .broker(
                binding,
                "test-forward",
                "https://example.test/mcp",
                CredentialPurpose::Invocation,
                &UseScope::default(),
            )
            .await?;
        assert_eq!(broker.cache().len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn secure_cached_credentials_cannot_be_reused_for_plaintext_binding() -> Result<()> {
        if run_isolated(
            "credentials::tests::secure_cached_credentials_cannot_be_reused_for_plaintext_binding",
        )? {
            return Ok(());
        }
        let resolvers = ResolverRegistry::new().with_resolver(Arc::new(HttpsEnvResolver::new()));
        let broker = CredentialBroker::new(resolvers);
        let mut binding = cached_binding()?;
        populate_https_cache(&broker, &binding).await?;
        binding.origin = NormalizedOrigin::from_address("http://example.test")?;
        for purpose in [CredentialPurpose::Discovery, CredentialPurpose::Invocation] {
            assert!(
                broker
                    .broker(
                        &binding,
                        "test-forward",
                        "http://example.test/mcp",
                        purpose,
                        &UseScope::default(),
                    )
                    .await
                    .is_err()
            );
        }
        assert_eq!(broker.cache().len(), 1);
        Ok(())
    }
}
