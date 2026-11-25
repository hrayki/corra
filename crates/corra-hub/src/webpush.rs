use aes_gcm::aead::Aead;
use aes_gcm::{Aes128Gcm, KeyInit};
use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use hkdf::Hkdf;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::{PublicKey, SecretKey};
use sha2::Sha256;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;
const RECORD_SIZE: u32 = 4096;

pub fn b64(bytes: &[u8]) -> String {
    B64.encode(bytes)
}

pub fn unb64(s: &str) -> Result<Vec<u8>> {
    B64.decode(s.trim().trim_end_matches('='))
        .context("not base64url")
}

pub fn new_key() -> SecretKey {
    loop {
        let bytes: [u8; 32] = rand::random();
        if let Ok(k) = SecretKey::from_slice(&bytes) {
            return k;
        }
    }
}

pub fn public_bytes(key: &SecretKey) -> Vec<u8> {
    key.public_key().to_sec1_bytes().to_vec()
}

fn expand(prk: &Hkdf<Sha256>, info: &[&[u8]], out: &mut [u8]) -> Result<()> {
    prk.expand_multi_info(info, out)
        .map_err(|_| anyhow!("hkdf output too long"))
}

pub fn encrypt(
    ua_public: &[u8],
    auth_secret: &[u8],
    payload: &[u8],
    as_key: &SecretKey,
    salt: [u8; 16],
) -> Result<Vec<u8>> {
    if payload.len() + 17 > RECORD_SIZE as usize - 86 {
        bail!("the notification is too long");
    }
    let ua =
        PublicKey::from_sec1_bytes(ua_public).context("the browser's key isn't a P-256 key")?;
    let as_public = public_bytes(as_key);
    let shared = p256::ecdh::diffie_hellman(as_key.to_nonzero_scalar(), ua.as_affine());
    let key_hk = Hkdf::<Sha256>::new(Some(auth_secret), shared.raw_secret_bytes());
    let mut ikm = [0u8; 32];
    expand(
        &key_hk,
        &[b"WebPush: info\0", ua_public, &as_public],
        &mut ikm,
    )?;
    let hk = Hkdf::<Sha256>::new(Some(&salt), &ikm);
    let mut cek = [0u8; 16];
    expand(&hk, &[b"Content-Encoding: aes128gcm\0"], &mut cek)?;
    let mut nonce = [0u8; 12];
    expand(&hk, &[b"Content-Encoding: nonce\0"], &mut nonce)?;
    let mut record = payload.to_vec();
    record.push(2);
    let sealed = Aes128Gcm::new_from_slice(&cek)
        .map_err(|_| anyhow!("bad content key"))?
        .encrypt(&nonce.into(), record.as_slice())
        .map_err(|_| anyhow!("couldn't encrypt the notification"))?;
    let mut body = Vec::with_capacity(86 + sealed.len());
    body.extend_from_slice(&salt);
    body.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    body.push(as_public.len() as u8);
    body.extend_from_slice(&as_public);
    body.extend_from_slice(&sealed);
    Ok(body)
}

pub fn vapid(key: &SecretKey, endpoint: &str, subject: &str, now_s: i64) -> Result<String> {
    let url = reqwest::Url::parse(endpoint).context("the push endpoint isn't a URL")?;
    let audience = url.origin().ascii_serialization();
    let header = b64(br#"{"typ":"JWT","alg":"ES256"}"#);
    let claims = b64(
        serde_json::json!({ "aud": audience, "exp": now_s + 12 * 3600, "sub": subject })
            .to_string()
            .as_bytes(),
    );
    let unsigned = format!("{header}.{claims}");
    let sig: Signature = SigningKey::from(key).sign(unsigned.as_bytes());
    Ok(format!(
        "vapid t={unsigned}.{}, k={}",
        b64(&sig.to_bytes()),
        b64(&public_bytes(key))
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc8291_example_encrypts_to_the_published_body() {
        let as_key =
            SecretKey::from_slice(&unb64("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw").unwrap())
                .unwrap();
        assert_eq!(
            b64(&public_bytes(&as_key)),
            "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8"
        );
        let ua_public = unb64(
            "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4",
        )
        .unwrap();
        let auth = unb64("BTBZMqHH6r4Tts7J_aSIgg").unwrap();
        let salt: [u8; 16] = unb64("DGv6ra1nlYgDCS1FRnbzlw").unwrap().try_into().unwrap();
        let body = encrypt(
            &ua_public,
            &auth,
            b"When I grow up, I want to be a watermelon",
            &as_key,
            salt,
        )
        .unwrap();
        assert_eq!(
            b64(&body),
            "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN"
        );
    }

    #[test]
    fn vapid_token_verifies_with_the_public_key() {
        use p256::ecdsa::VerifyingKey;
        use p256::ecdsa::signature::Verifier;
        let key = new_key();
        let h = vapid(
            &key,
            "https://fcm.googleapis.com/fcm/send/abc",
            "https://hub.example",
            1_000,
        )
        .unwrap();
        let rest = h.strip_prefix("vapid t=").unwrap();
        let (token, k) = rest.split_once(", k=").unwrap();
        let (unsigned, sig) = token.rsplit_once('.').unwrap();
        let claims: serde_json::Value =
            serde_json::from_slice(&unb64(unsigned.split('.').nth(1).unwrap()).unwrap()).unwrap();
        assert_eq!(claims["aud"], "https://fcm.googleapis.com");
        assert_eq!(claims["exp"], 1_000 + 12 * 3600);
        let vk = VerifyingKey::from_sec1_bytes(&unb64(k).unwrap()).unwrap();
        let sig = Signature::from_slice(&unb64(sig).unwrap()).unwrap();
        vk.verify(unsigned.as_bytes(), &sig).unwrap();
    }
}
