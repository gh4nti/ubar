use aes::Aes128;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ctr::cipher::{KeyIvInit, StreamCipher};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use zeroize::{Zeroize, ZeroizeOnDrop};

pub mod widevine;
pub mod broker;

type Aes128Ctr = ctr::Ctr128BE<Aes128>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionType {
    Temporary,
    PersistentLicense,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionState {
    New,
    AwaitingLicense,
    Usable,
    Closed,
}

#[derive(Clone, Debug, Eq, PartialEq, Zeroize, ZeroizeOnDrop)]
pub struct ContentKey([u8; 16]);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KeyStatus {
    Usable,
    InternalError,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LicenseRequest {
    pub init_data_type: String,
    pub message: Vec<u8>,
}

#[derive(Debug)]
pub struct ClearKeySession {
    id: String,
    session_type: SessionType,
    state: SessionState,
    keys: BTreeMap<Vec<u8>, ContentKey>,
}

#[derive(Deserialize)]
struct JwkSet {
    keys: Vec<JwkKey>,
    #[serde(default)]
    r#type: Option<String>,
}

#[derive(Deserialize)]
struct JwkKey {
    kty: String,
    kid: String,
    k: String,
}

#[derive(Serialize)]
struct KeyIdsRequest<'a> {
    kids: Vec<&'a str>,
    r#type: &'a str,
}

impl ClearKeySession {
    pub fn new(id: impl Into<String>, session_type: SessionType) -> Self {
        Self {
            id: id.into(),
            session_type,
            state: SessionState::New,
            keys: BTreeMap::new(),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn state(&self) -> SessionState {
        self.state
    }

    pub fn generate_request(
        &mut self,
        init_data_type: &str,
        init_data: &[u8],
    ) -> Result<LicenseRequest, String> {
        if self.state != SessionState::New {
            return Err("generateRequest is only valid for a new session".into());
        }
        let message = match init_data_type {
            "keyids" => {
                let value: serde_json::Value =
                    serde_json::from_slice(init_data).map_err(|error| error.to_string())?;
                let kids = value
                    .get("kids")
                    .and_then(serde_json::Value::as_array)
                    .ok_or("keyids init data has no kids")?
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .collect::<Vec<_>>();
                if kids.is_empty() {
                    return Err("keyids init data is empty".into());
                }
                serde_json::to_vec(&KeyIdsRequest {
                    kids,
                    r#type: match self.session_type {
                        SessionType::Temporary => "temporary",
                        SessionType::PersistentLicense => "persistent-license",
                    },
                })
                .map_err(|error| error.to_string())?
            }
            "cenc" | "webm" => init_data.to_vec(),
            _ => return Err(format!("unsupported init data type: {init_data_type}")),
        };
        self.state = SessionState::AwaitingLicense;
        Ok(LicenseRequest {
            init_data_type: init_data_type.into(),
            message,
        })
    }

    pub fn update(&mut self, response: &[u8]) -> Result<Vec<(Vec<u8>, KeyStatus)>, String> {
        if self.state != SessionState::AwaitingLicense && self.state != SessionState::Usable {
            return Err("update is not valid in the current session state".into());
        }
        let jwks: JwkSet = serde_json::from_slice(response).map_err(|error| error.to_string())?;
        if let Some(kind) = jwks.r#type.as_deref() {
            let expected = match self.session_type {
                SessionType::Temporary => "temporary",
                SessionType::PersistentLicense => "persistent-license",
            };
            if kind != expected {
                return Err(format!("license type {kind} does not match {expected}"));
            }
        }
        if jwks.keys.is_empty() {
            return Err("license contains no keys".into());
        }
        let mut statuses = Vec::new();
        for key in jwks.keys {
            if key.kty != "oct" {
                return Err("ClearKey only accepts oct JWK keys".into());
            }
            let kid = URL_SAFE_NO_PAD
                .decode(key.kid)
                .map_err(|error| format!("invalid kid: {error}"))?;
            let raw = URL_SAFE_NO_PAD
                .decode(key.k)
                .map_err(|error| format!("invalid key: {error}"))?;
            let raw: [u8; 16] = raw
                .try_into()
                .map_err(|_| "ClearKey content keys must be 128-bit")?;
            self.keys.insert(kid.clone(), ContentKey(raw));
            statuses.push((kid, KeyStatus::Usable));
        }
        self.state = SessionState::Usable;
        Ok(statuses)
    }

    pub fn decrypt_aes_ctr(
        &self,
        key_id: &[u8],
        counter_block: &[u8; 16],
        data: &mut [u8],
    ) -> Result<(), String> {
        if self.state != SessionState::Usable {
            return Err("session has no usable keys".into());
        }
        let key = self.keys.get(key_id).ok_or("key ID is not licensed")?;
        let mut cipher = Aes128Ctr::new((&key.0).into(), counter_block.into());
        cipher.apply_keystream(data);
        Ok(())
    }

    pub fn close(&mut self) {
        self.keys.clear();
        self.state = SessionState::Closed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(value: &str) -> Vec<u8> {
        value
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap()
            })
            .collect()
    }

    #[test]
    fn clear_key_license_decrypts_nist_aes_ctr_vector() {
        let key = hex("2b7e151628aed2a6abf7158809cf4f3c");
        let kid = hex("00112233445566778899aabbccddeeff");
        let license = serde_json::json!({
            "keys": [{
                "kty": "oct",
                "kid": URL_SAFE_NO_PAD.encode(&kid),
                "k": URL_SAFE_NO_PAD.encode(&key)
            }],
            "type": "temporary"
        });
        let mut session = ClearKeySession::new("test", SessionType::Temporary);
        session
            .generate_request("keyids", br#"{"kids":["ABEiM0RVZneImaq7zN3u_w"]}"#)
            .unwrap();
        session.update(&serde_json::to_vec(&license).unwrap()).unwrap();
        let mut data = hex("6bc1bee22e409f96e93d7e117393172a");
        let counter: [u8; 16] = hex("f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff")
            .try_into()
            .unwrap();
        session.decrypt_aes_ctr(&kid, &counter, &mut data).unwrap();
        assert_eq!(data, hex("874d6191b620e3261bef6864990db6ce"));
    }

    #[test]
    fn close_zeroizes_and_rejects_decryption() {
        let mut session = ClearKeySession::new("test", SessionType::Temporary);
        session.close();
        assert_eq!(session.state(), SessionState::Closed);
        assert!(session
            .decrypt_aes_ctr(&[], &[0; 16], &mut [0; 16])
            .is_err());
    }

    #[test]
    fn rejects_license_type_mismatch() {
        let mut session = ClearKeySession::new("test", SessionType::PersistentLicense);
        session
            .generate_request("keyids", br#"{"kids":["ABEiM0RVZneImaq7zN3u_w"]}"#)
            .unwrap();
        let license = br#"{"keys":[],"type":"temporary"}"#;
        assert!(session.update(license).is_err());
    }
}
