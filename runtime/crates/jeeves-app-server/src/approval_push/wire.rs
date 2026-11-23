//! RFC 8291 single-record aes128gcm and RFC 8292 VAPID, using the workspace's ring primitives.
use anyhow::{Result, anyhow, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use ring::{
    aead, agreement, hkdf,
    rand::{SecureRandom, SystemRandom},
    signature::{EcdsaKeyPair, KeyPair},
};
use serde_json::json;

struct Size(usize);
impl hkdf::KeyType for Size {
    fn len(&self) -> usize {
        self.0
    }
}
fn expand<const N: usize>(salt: &[u8], secret: &[u8], info: &[&[u8]]) -> Result<[u8; N]> {
    let mut out = [0; N];
    hkdf::Salt::new(hkdf::HKDF_SHA256, salt)
        .extract(secret)
        .expand(info, Size(N))
        .and_then(|key| key.fill(&mut out))
        .map_err(|_| anyhow!("Web Push key derivation failed"))?;
    Ok(out)
}
pub(super) fn keys(
    shared: &[u8],
    receiver: &[u8],
    sender: &[u8],
    auth: &[u8],
    salt: &[u8],
) -> Result<([u8; 16], [u8; 12])> {
    let ikm = expand::<32>(auth, shared, &[b"WebPush: info\0", receiver, sender])?;
    Ok((
        expand(salt, &ikm, &[b"Content-Encoding: aes128gcm\0"])?,
        expand(salt, &ikm, &[b"Content-Encoding: nonce\0"])?,
    ))
}
pub(super) fn encrypt(receiver: &[u8], auth: &[u8], mut plain: Vec<u8>) -> Result<Vec<u8>> {
    ensure!(plain.len() <= 3993, "Notification is too large");
    let rng = SystemRandom::new();
    let fail = |_| anyhow!("Web Push encryption failed");
    let private =
        agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng).map_err(fail)?;
    let public = private.compute_public_key().map_err(fail)?;
    let peer = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, receiver);
    let mut salt = [0; 16];
    rng.fill(&mut salt).map_err(fail)?;
    let (key, nonce) = agreement::agree_ephemeral(private, &peer, |shared| {
        keys(shared, receiver, public.as_ref(), auth, &salt)
    })
    .map_err(fail)??;
    plain.push(2);
    aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_128_GCM, &key).map_err(fail)?)
        .seal_in_place_append_tag(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::empty(),
            &mut plain,
        )
        .map_err(fail)?;
    let mut body = salt.to_vec();
    body.extend_from_slice(&4096u32.to_be_bytes());
    body.push(public.as_ref().len() as u8);
    body.extend_from_slice(public.as_ref());
    body.extend(plain);
    Ok(body)
}
pub(super) fn authorization(
    key: &EcdsaKeyPair,
    endpoint: &reqwest::Url,
    subject: &str,
) -> Result<String> {
    let claims = json!({"aud":endpoint.origin().ascii_serialization(),"exp":chrono::Utc::now().timestamp()+3600,"sub":subject});
    let signed = format!(
        "{}.{}",
        B64.encode(br#"{"typ":"JWT","alg":"ES256"}"#),
        B64.encode(serde_json::to_vec(&claims)?)
    );
    let signature = key
        .sign(&SystemRandom::new(), signed.as_bytes())
        .map_err(|_| anyhow!("VAPID signing failed"))?;
    Ok(format!(
        "vapid t={signed}.{}, k={}",
        B64.encode(signature.as_ref()),
        B64.encode(key.public_key().as_ref())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rfc8291_appendix_a_keys() {
        let bytes = |s| B64.decode(s).unwrap();
        let (key,nonce)=keys(&bytes("kyrL1jIIOHEzg3sM2ZWRHDRB62YACZhhSlknJ672kSs"),&bytes("BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4"),&bytes("BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8"),&bytes("BTBZMqHH6r4Tts7J_aSIgg"),&bytes("DGv6ra1nlYgDCS1FRnbzlw")).unwrap();
        assert_eq!(B64.encode(key), "oIhVW04MRdy2XN9CiKLxTg");
        assert_eq!(B64.encode(nonce), "4h_95klXJ5E_qnoN");
    }
}
