//! Tools for generating self-signed certificates (testing purposes only)

use std::marker::PhantomData;

/// A tool for generating self-signed certificates (testing purposes only)
pub struct MockTls {
    server_ca: Ca,
    client_ca: Ca,
}

impl MockTls {
    pub fn generate() -> Self {
        let server_ca = Ca::generate();
        let client_ca = Ca::generate();

        Self {
            server_ca,
            client_ca,
        }
    }

    pub fn issue_server_cert(
        &self,
        server_host: Vec<String>,
    ) -> MockedCertificate<rustls::ServerConfig> {
        let cert = self
            .server_ca
            .issue_cert(rcgen::ExtendedKeyUsagePurpose::ServerAuth, server_host);
        let cert_chain = vec![rustls::Certificate(
            cert.serialize_der_with_signer(&self.server_ca.certificate)
                .unwrap(),
        )];
        let private_key = rustls::PrivateKey(cert.serialize_private_key_der());

        MockedCertificate {
            certificate: rustls::Certificate(cert.serialize_der().unwrap()),
            cert_chain,
            private_key,
            ca: &self.client_ca,
            _ph: PhantomData,
        }
    }

    pub fn issue_client_cert(
        &self,
        client_alt_name: Vec<String>,
    ) -> MockedCertificate<rustls::ClientConfig> {
        let cert = self
            .client_ca
            .issue_cert(rcgen::ExtendedKeyUsagePurpose::ClientAuth, client_alt_name);
        let cert_chain = vec![rustls::Certificate(
            cert.serialize_der_with_signer(&self.client_ca.certificate)
                .unwrap(),
        )];
        let private_key = rustls::PrivateKey(cert.serialize_private_key_der());

        MockedCertificate {
            certificate: rustls::Certificate(cert.serialize_der().unwrap()),
            cert_chain,
            private_key,
            ca: &self.server_ca,
            _ph: PhantomData,
        }
    }
}

struct Ca {
    certificate: rcgen::Certificate,
}

impl Ca {
    pub fn generate() -> Self {
        let mut ca_params = rcgen::CertificateParams::default();
        ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        ca_params.key_usages = vec![rcgen::KeyUsagePurpose::KeyCertSign];
        let ca = rcgen::Certificate::from_params(ca_params).unwrap();
        Self { certificate: ca }
    }

    pub fn ca_cert(&self) -> rustls::Certificate {
        rustls::Certificate(self.certificate.serialize_der().unwrap())
    }

    pub fn issue_cert(
        &self,
        purpose: rcgen::ExtendedKeyUsagePurpose,
        alt_names: Vec<String>,
    ) -> rcgen::Certificate {
        let mut cert_params = rcgen::CertificateParams::new(alt_names);
        cert_params.key_usages = vec![
            rcgen::KeyUsagePurpose::DigitalSignature,
            rcgen::KeyUsagePurpose::KeyAgreement,
        ];
        cert_params.extended_key_usages = vec![purpose];
        rcgen::Certificate::from_params(cert_params).unwrap()
    }
}

/// TLS certificate generated by [`MockTls`]
pub struct MockedCertificate<'ca, C> {
    certificate: rustls::Certificate,
    cert_chain: Vec<rustls::Certificate>,
    private_key: rustls::PrivateKey,
    ca: &'ca Ca,
    _ph: PhantomData<C>,
}
impl<'ca, C> MockedCertificate<'ca, C> {
    pub fn certificate(&self) -> &rustls::Certificate {
        &self.certificate
    }
    pub fn cert_chain(&self) -> &[rustls::Certificate] {
        &self.cert_chain
    }
    pub fn private_key(&self) -> &rustls::PrivateKey {
        &self.private_key
    }
}
impl<'ca> MockedCertificate<'ca, rustls::ServerConfig> {
    pub fn derive_server_config(&self) -> rustls::ServerConfig {
        let mut root_certs = rustls::RootCertStore::empty();
        root_certs.add(&self.ca.ca_cert()).unwrap();

        rustls::ServerConfig::builder()
            .with_safe_default_cipher_suites()
            .with_safe_default_kx_groups()
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_client_cert_verifier(rustls::server::AllowAnyAuthenticatedClient::new(root_certs))
            .with_single_cert(self.cert_chain.clone(), self.private_key.clone())
            .unwrap()
    }
}
impl<'ca> MockedCertificate<'ca, rustls::ClientConfig> {
    pub fn derive_client_config(&self) -> rustls::ClientConfig {
        let mut root_certs = rustls::RootCertStore::empty();
        root_certs.add(&self.ca.ca_cert()).unwrap();

        rustls::ClientConfig::builder()
            .with_safe_default_cipher_suites()
            .with_safe_default_kx_groups()
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_root_certificates(root_certs)
            .with_single_cert(self.cert_chain.clone(), self.private_key.clone())
            .unwrap()
    }
}