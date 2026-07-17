use openssl::hash::MessageDigest;
use openssl::nid::Nid;
use openssl::pkey::{Id, PKey};
use openssl::rsa::Padding;
use openssl::sign::Verifier;
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::path::PathBuf;
use ubar_extension_host::package::{
    InstallPolicy, PackageKind, PackageSignatureVerifier, VerifiedIdentity, inspect_package,
};

const MAX_PACKAGE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_PROOFS: usize = 64;
const MAX_KEY_OR_SIGNATURE_BYTES: usize = 64 * 1024;
const SIGNATURE_CONTEXT: &[u8] = b"CRX3 SignedData\0";

#[derive(Clone, Copy)]
enum Algorithm { Rsa, Ecdsa }

struct Proof<'a> {
    algorithm: Algorithm,
    public_key: &'a [u8],
    signature: &'a [u8],
}

struct CrxHeader<'a> {
    signed_data: &'a [u8],
    proofs: Vec<Proof<'a>>,
}

struct Crx3Verifier;

impl PackageSignatureVerifier for Crx3Verifier {
    fn verify_xpi(
        &self,
        _: &[u8],
        _: &[u8],
        _: &[u8],
        _: &[u8],
    ) -> Result<VerifiedIdentity, String> {
        Err("XPI verification uses the Mozilla verifier".into())
    }

    fn verify_crx3(&self, header: &[u8], archive: &[u8]) -> Result<VerifiedIdentity, String> {
        let header = parse_header(header)?;
        let crx_id = parse_signed_data(header.signed_data)?;
        let extension_id = id_from_bytes(crx_id);
        let signed_data_size = u32::try_from(header.signed_data.len())
            .map_err(|_| "CRX3 signed header is too large")?.to_le_bytes();
        let mut developer_key = None;

        if header.proofs.is_empty() { return Err("CRX3 contains no signature proofs".into()); }
        for proof in &header.proofs {
            let key_hash = Sha256::digest(proof.public_key);
            if key_hash[..16] == *crx_id && developer_key.is_none() {
                developer_key = Some((proof.algorithm, key_hash));
            }
            verify_proof(proof, &signed_data_size, header.signed_data, archive)?;
        }

        let (algorithm, key_hash) = developer_key
            .ok_or("CRX3 has no verified developer key matching its ID")?;
        Ok(VerifiedIdentity {
            extension_id,
            signer: match algorithm {
                Algorithm::Rsa => "CRX3 RSA developer key",
                Algorithm::Ecdsa => "CRX3 ECDSA P-256 developer key",
            }.into(),
            certificate_fingerprint_sha256: hex(&key_hash),
        })
    }
}

fn verify_proof(
    proof: &Proof<'_>,
    signed_data_size: &[u8; 4],
    signed_data: &[u8],
    archive: &[u8],
) -> Result<(), String> {
    let key = PKey::public_key_from_der(proof.public_key)
        .map_err(|error| format!("CRX3 public key is invalid: {error}"))?;
    match proof.algorithm {
        Algorithm::Rsa if key.id() == Id::RSA => {}
        Algorithm::Ecdsa if key.id() == Id::EC => {
            if key.ec_key().map_err(|error| error.to_string())?.group().curve_name()
                != Some(Nid::X9_62_PRIME256V1) {
                return Err("CRX3 ECDSA key is not NIST P-256".into());
            }
        }
        Algorithm::Rsa => return Err("CRX3 RSA proof does not contain an RSA key".into()),
        Algorithm::Ecdsa => return Err("CRX3 ECDSA proof does not contain an EC key".into()),
    }
    let mut verifier = Verifier::new(MessageDigest::sha256(), &key)
        .map_err(|error| error.to_string())?;
    if matches!(proof.algorithm, Algorithm::Rsa) {
        verifier.set_rsa_padding(Padding::PKCS1).map_err(|error| error.to_string())?;
    }
    for bytes in [SIGNATURE_CONTEXT, signed_data_size, signed_data, archive] {
        verifier.update(bytes).map_err(|error| error.to_string())?;
    }
    if !verifier.verify(proof.signature).map_err(|error| error.to_string())? {
        return Err("CRX3 signature verification failed".into());
    }
    Ok(())
}

fn parse_header(bytes: &[u8]) -> Result<CrxHeader<'_>, String> {
    let mut reader = ProtoReader::new(bytes);
    let mut signed_data = None;
    let mut proofs = Vec::new();
    while let Some(field) = reader.next()? {
        match field.number {
            2 | 3 => {
                if proofs.len() == MAX_PROOFS { return Err("CRX3 has too many proofs".into()); }
                proofs.push(parse_proof(
                    field.bytes()?,
                    if field.number == 2 { Algorithm::Rsa } else { Algorithm::Ecdsa },
                )?);
            }
            10_000 => {
                if signed_data.replace(field.bytes()?).is_some() {
                    return Err("CRX3 has duplicate signed_header_data".into());
                }
            }
            _ => {}
        }
    }
    Ok(CrxHeader {
        signed_data: signed_data.ok_or("CRX3 has no signed_header_data")?,
        proofs,
    })
}

fn parse_proof(bytes: &[u8], algorithm: Algorithm) -> Result<Proof<'_>, String> {
    let mut reader = ProtoReader::new(bytes);
    let mut public_key = None;
    let mut signature = None;
    while let Some(field) = reader.next()? {
        match field.number {
            1 => {
                if public_key.replace(field.bytes()?).is_some() {
                    return Err("CRX3 proof has duplicate public key".into());
                }
            }
            2 => {
                if signature.replace(field.bytes()?).is_some() {
                    return Err("CRX3 proof has duplicate signature".into());
                }
            }
            _ => {}
        }
    }
    let public_key = public_key.filter(|value| !value.is_empty())
        .ok_or("CRX3 proof has no public key")?;
    let signature = signature.filter(|value| !value.is_empty())
        .ok_or("CRX3 proof has no signature")?;
    if public_key.len() > MAX_KEY_OR_SIGNATURE_BYTES || signature.len() > MAX_KEY_OR_SIGNATURE_BYTES {
        return Err("CRX3 proof key or signature is too large".into());
    }
    Ok(Proof { algorithm, public_key, signature })
}

fn parse_signed_data(bytes: &[u8]) -> Result<&[u8; 16], String> {
    let mut reader = ProtoReader::new(bytes);
    let mut id = None;
    while let Some(field) = reader.next()? {
        if field.number == 1 && id.replace(field.bytes()?).is_some() {
            return Err("CRX3 SignedData has duplicate crx_id".into());
        }
    }
    id.ok_or("CRX3 SignedData has no crx_id")?.try_into()
        .map_err(|_| "CRX3 crx_id must be exactly 16 bytes".into())
}

struct ProtoField<'a> {
    number: u32,
    wire_type: u8,
    data: Option<&'a [u8]>,
}

impl<'a> ProtoField<'a> {
    fn bytes(self) -> Result<&'a [u8], String> {
        if self.wire_type != 2 { return Err("CRX3 protobuf field has wrong wire type".into()); }
        self.data.ok_or("CRX3 protobuf byte field is missing".into())
    }
}

struct ProtoReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> ProtoReader<'a> {
    fn new(bytes: &'a [u8]) -> Self { Self { bytes, offset: 0 } }

    fn next(&mut self) -> Result<Option<ProtoField<'a>>, String> {
        if self.offset == self.bytes.len() { return Ok(None); }
        let tag = self.varint()?;
        let number = u32::try_from(tag >> 3).map_err(|_| "CRX3 protobuf field number overflow")?;
        if number == 0 { return Err("CRX3 protobuf contains field zero".into()); }
        let wire_type = (tag & 7) as u8;
        let data = match wire_type {
            0 => { self.varint()?; None }
            1 => { self.take(8)?; None }
            2 => {
                let length = usize::try_from(self.varint()?)
                    .map_err(|_| "CRX3 protobuf length overflow")?;
                Some(self.take(length)?)
            }
            5 => { self.take(4)?; None }
            _ => return Err("CRX3 protobuf uses unsupported wire type".into()),
        };
        Ok(Some(ProtoField { number, wire_type, data }))
    }

    fn varint(&mut self) -> Result<u64, String> {
        let mut value = 0u64;
        for index in 0..10 {
            let byte = *self.bytes.get(self.offset).ok_or("CRX3 protobuf is truncated")?;
            self.offset += 1;
            if index == 9 && byte > 1 { return Err("CRX3 protobuf varint overflow".into()); }
            value |= u64::from(byte & 0x7f) << (index * 7);
            if byte & 0x80 == 0 { return Ok(value); }
        }
        Err("CRX3 protobuf varint overflow".into())
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], String> {
        let end = self.offset.checked_add(length).ok_or("CRX3 protobuf length overflow")?;
        let value = self.bytes.get(self.offset..end).ok_or("CRX3 protobuf is truncated")?;
        self.offset = end;
        Ok(value)
    }
}

fn id_from_bytes(bytes: &[u8]) -> String {
    bytes.iter().flat_map(|byte| [byte >> 4, byte & 15])
        .map(|nibble| char::from(b'a' + nibble)).collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn main() {
    if let Err(error) = run() {
        eprintln!("ubar-crx3-verifier: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let mut arguments = env::args_os().skip(1);
    let path = arguments.next().map(PathBuf::from)
        .ok_or("usage: ubar-crx3-verifier PACKAGE.crx")?;
    if arguments.next().is_some() { return Err("usage: ubar-crx3-verifier PACKAGE.crx".into()); }
    let path = path.canonicalize().map_err(|error| error.to_string())?;
    let metadata = fs::metadata(&path).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_PACKAGE_BYTES {
        return Err("CRX3 package size is invalid".into());
    }
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    let package = inspect_package(&bytes, &InstallPolicy::default(), &Crx3Verifier)?;
    if package.kind != PackageKind::ChromeCrx3 || package.identity.is_none() {
        return Err("package is not a signed CRX3 extension".into());
    }
    serde_json::to_writer(std::io::stdout().lock(), &package).map_err(|error| error.to_string())
}
