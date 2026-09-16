use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use http::uri::Authority;
use hudsucker::certificate_authority::CertificateAuthority;
use hudsucker::rustls::crypto::CryptoProvider;
use hudsucker::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use hudsucker::rustls::ServerConfig;
use rand::{rng, RngExt};
use rcgen::{
    CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, Issuer, KeyPair,
    KeyUsagePurpose, SanType,
};
use time::{Duration, OffsetDateTime};

const CERTIFICATE_TTL_SECONDS: i64 = 365 * 24 * 60 * 60;
const NOT_BEFORE_OFFSET_SECONDS: i64 = 60;

/// Hudsucker's rcgen authority leaves Extended Key Usage unspecified on the
/// certificates it mints. Most TLS clients accept that, but Copilot's native
/// rustls platform verifier requires an explicit `serverAuth` purpose.
pub(super) struct ServerAuthAuthority {
    issuer: Issuer<'static, KeyPair>,
    private_key: PrivateKeyDer<'static>,
    cache: Mutex<HashMap<Authority, Arc<ServerConfig>>>,
    provider: Arc<CryptoProvider>,
}

impl ServerAuthAuthority {
    pub(super) fn new(issuer: Issuer<'static, KeyPair>, provider: CryptoProvider) -> Self {
        let private_key =
            PrivateKeyDer::from(PrivatePkcs8KeyDer::from(issuer.key().serialize_der()));

        Self {
            issuer,
            private_key,
            cache: Mutex::new(HashMap::new()),
            provider: Arc::new(provider),
        }
    }

    fn certificate_params(authority: &Authority) -> CertificateParams {
        let mut params = CertificateParams::default();
        params.serial_number = Some(rng().random::<u64>().into());

        let not_before =
            OffsetDateTime::now_utc() - Duration::seconds(NOT_BEFORE_OFFSET_SECONDS);
        params.not_before = not_before;
        params.not_after = not_before + Duration::seconds(CERTIFICATE_TTL_SECONDS);

        let mut distinguished_name = DistinguishedName::new();
        distinguished_name.push(DnType::CommonName, authority.host());
        params.distinguished_name = distinguished_name;
        params.subject_alt_names.push(SanType::DnsName(
            authority
                .host()
                .try_into()
                .expect("relay hosts must be valid DNS names"),
        ));
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params
    }

    fn gen_cert(&self, authority: &Authority) -> CertificateDer<'static> {
        Self::certificate_params(authority)
            .signed_by(self.issuer.key(), &self.issuer)
            .expect("failed to sign relay certificate")
            .into()
    }
}

impl CertificateAuthority for ServerAuthAuthority {
    async fn gen_server_config(&self, authority: &Authority) -> Arc<ServerConfig> {
        if let Some(config) = self
            .cache
            .lock()
            .expect("relay certificate cache lock poisoned")
            .get(authority)
            .cloned()
        {
            return config;
        }

        let mut config = ServerConfig::builder_with_provider(Arc::clone(&self.provider))
            .with_safe_default_protocol_versions()
            .expect("failed to select TLS protocol versions")
            .with_no_client_auth()
            .with_single_cert(vec![self.gen_cert(authority)], self.private_key.clone_key())
            .expect("failed to build relay TLS config");
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        let config = Arc::new(config);

        self.cache
            .lock()
            .expect("relay certificate cache lock poisoned")
            .insert(authority.clone(), Arc::clone(&config));
        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaf_certificates_are_tls_server_certificates() {
        let params = ServerAuthAuthority::certificate_params(&Authority::from_static(
            "api.githubcopilot.com",
        ));

        assert_eq!(
            params.extended_key_usages,
            vec![ExtendedKeyUsagePurpose::ServerAuth]
        );
        assert_eq!(params.key_usages, vec![KeyUsagePurpose::DigitalSignature]);
    }
}
