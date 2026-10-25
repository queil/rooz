use pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rcgen::{CertificateParams, DnType, Issuer, KeyPair};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

// Certificates for the hosts the proxy intercepts, issued from the CA the clients trust and kept
// for as long as the proxy runs.
pub struct Mint {
    issuer: Issuer<'static, KeyPair>,
    ca: CertificateDer<'static>,
    issued: Mutex<HashMap<String, Arc<rustls::sign::CertifiedKey>>>,
}

impl Mint {
    pub fn load(cert_pem: &str, key_pem: &str) -> Result<Self, String> {
        let key = KeyPair::from_pem(key_pem).map_err(|e| format!("ca key: {}", e))?;
        let issuer =
            Issuer::from_ca_cert_pem(cert_pem, key).map_err(|e| format!("ca cert: {}", e))?;
        let ca = rustls::pki_types::pem::PemObject::from_pem_slice(cert_pem.as_bytes())
            .map_err(|e| format!("ca cert: {}", e))?;
        Ok(Self {
            issuer,
            ca,
            issued: Mutex::new(HashMap::new()),
        })
    }

    pub fn for_host(&self, host: &str) -> Result<Arc<rustls::sign::CertifiedKey>, String> {
        if let Some(k) = self.issued.lock().unwrap().get(host) {
            return Ok(k.clone());
        }
        let mut params = CertificateParams::new(vec![host.to_string()])
            .map_err(|e| format!("certificate for {}: {}", host, e))?;
        params.distinguished_name.push(DnType::CommonName, host);
        let key = KeyPair::generate().map_err(|e| e.to_string())?;
        let cert = params
            .signed_by(&key, &self.issuer)
            .map_err(|e| format!("certificate for {}: {}", host, e))?;
        let private = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
        let signer = rustls::crypto::ring::sign::any_supported_type(&private)
            .map_err(|e| format!("key for {}: {}", host, e))?;
        let issued = Arc::new(rustls::sign::CertifiedKey::new(
            vec![cert.der().clone(), self.ca.clone()],
            signer,
        ));
        self.issued
            .lock()
            .unwrap()
            .insert(host.to_string(), issued.clone());
        Ok(issued)
    }
}
