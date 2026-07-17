use openssl::hash::MessageDigest;
use openssl::pkcs7::{Pkcs7, Pkcs7Flags};
use openssl::stack::Stack;
use openssl::x509::{X509, X509StoreContext, store::X509StoreBuilder, verify::X509VerifyFlags};
use std::env;
use std::fs;
use std::path::PathBuf;
use ubar_extension_host::package::{
    InstallPolicy, PackageSignatureVerifier, VerifiedIdentity, inspect_package,
};

const MAX_PACKAGE_BYTES: u64 = 512 * 1024 * 1024;
const MOZILLA_ADDONS_ROOT: &[u8] = include_bytes!("../../../config/trust/mozilla-addons-public.pem");
const MOZILLA_ADDONS_INTERMEDIATES: &[u8] =
    include_bytes!("../../../config/trust/mozilla-addons-public-intermediates.pem");

struct MozillaVerifier;

impl PackageSignatureVerifier for MozillaVerifier {
    fn verify_xpi(
        &self,
        _package: &[u8],
        _manifest_mf: &[u8],
        signature_file: &[u8],
        signature_block: &[u8],
    ) -> Result<VerifiedIdentity, String> {
        let pkcs7 = Pkcs7::from_der(signature_block).map_err(|error| error.to_string())?;
        let root = X509::from_pem(MOZILLA_ADDONS_ROOT).map_err(|error| error.to_string())?;
        let mut store = X509StoreBuilder::new().map_err(|error| error.to_string())?;
        store.add_cert(root).map_err(|error| error.to_string())?;
        store.set_flags(X509VerifyFlags::NO_CHECK_TIME).map_err(|error| error.to_string())?;
        let store = store.build();
        let certificates = Stack::new().map_err(|error| error.to_string())?;
        pkcs7.verify(
            &certificates,
            &store,
            Some(signature_file),
            None,
            Pkcs7Flags::BINARY | Pkcs7Flags::NOVERIFY,
        ).map_err(|error| format!("Mozilla XPI CMS verification failed: {error}"))?;
        let signers = pkcs7.signers(&certificates, Pkcs7Flags::empty())
            .map_err(|error| error.to_string())?;
        if signers.len() != 1 { return Err("XPI must have exactly one signer".into()); }
        let certificate = &signers[0];
        let mut intermediates = Stack::new().map_err(|error| error.to_string())?;
        for intermediate in X509::stack_from_pem(MOZILLA_ADDONS_INTERMEDIATES)
            .map_err(|error| error.to_string())? {
            intermediates.push(intermediate).map_err(|error| error.to_string())?;
        }
        let mut context = X509StoreContext::new().map_err(|error| error.to_string())?;
        context.init(&store, certificate, &intermediates, |context| context.verify_cert())
            .map_err(|error| format!("Mozilla XPI certificate chain failed: {error}"))?;
        let extension_id = certificate.subject_name().entries_by_nid(openssl::nid::Nid::COMMONNAME)
            .next().ok_or("XPI signer certificate has no common name")?
            .data().as_utf8().map_err(|error| error.to_string())?.to_string();
        if extension_id.is_empty() { return Err("XPI signer extension ID is empty".into()); }
        let signer = certificate.subject_name().entries()
            .filter_map(|entry| entry.data().as_utf8().ok().map(|value| value.to_string()))
            .collect::<Vec<_>>().join(", ");
        let fingerprint = certificate.digest(MessageDigest::sha256())
            .map_err(|error| error.to_string())?
            .iter().map(|byte| format!("{byte:02x}")).collect();
        Ok(VerifiedIdentity {
            extension_id,
            signer,
            certificate_fingerprint_sha256: fingerprint,
        })
    }

    fn verify_crx3(&self, _: &[u8], _: &[u8]) -> Result<VerifiedIdentity, String> {
        Err("CRX3 verification uses a separate verifier".into())
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("ubar-xpi-verifier: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let path = env::args_os().nth(1).map(PathBuf::from)
        .ok_or("usage: ubar-xpi-verifier PACKAGE.xpi")?;
    let metadata = fs::metadata(&path).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_PACKAGE_BYTES {
        return Err("XPI package size is invalid".into());
    }
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    let package = inspect_package(&bytes, &InstallPolicy::default(), &MozillaVerifier)?;
    serde_json::to_writer(std::io::stdout().lock(), &package).map_err(|error| error.to_string())
}
