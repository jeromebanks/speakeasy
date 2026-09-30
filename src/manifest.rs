//! Signed publication manifest, format v1. The exact byte layout is specified
//! in docs/manifest-format.md; keep the two in sync.

use anyhow::{Context, Result, bail, ensure};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

use crate::paths::validate_path_set;

/// Domain-separation tag; the first bytes of every signed manifest.
pub const DOMAIN_TAG: &[u8; 22] = b"speakeasy manifest v1\n";
pub const SIGNATURE_LEN: usize = 64;
const FEED_ID_CONTEXT: &str = "speakeasy 2026-09 feed id v1";

/// Limits applied when creating or accepting a manifest.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_manifest_bytes: usize,
    pub max_artifacts: usize,
    pub max_artifact_size: u64,
    pub max_total_size: u64,
    pub max_text_len: usize,
    pub max_attributes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_manifest_bytes: 4 * 1024 * 1024,
            max_artifacts: 10_000,
            max_artifact_size: 4 * 1024 * 1024 * 1024,
            max_total_size: 16 * 1024 * 1024 * 1024,
            max_text_len: 4096,
            max_attributes: 64,
        }
    }
}

/// Stable feed identifier derived from the publisher key and feed name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FeedId(pub [u8; 32]);

impl FeedId {
    pub fn derive(publisher: &[u8; 32], feed: &str) -> Self {
        let mut h = blake3::Hasher::new_derive_key(FEED_ID_CONTEXT);
        h.update(publisher);
        h.update(feed.as_bytes());
        Self(*h.finalize().as_bytes())
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

impl std::fmt::Display for FeedId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// Feed names: 1..=64 bytes of `[a-z0-9._-]`, starting with a letter or digit.
pub fn validate_feed_name(name: &str) -> Result<()> {
    ensure!(
        (1..=64).contains(&name.len()),
        "feed name must be 1..=64 bytes"
    );
    ensure!(
        name.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-')),
        "feed name may contain only [a-z0-9._-]: {name:?}"
    );
    ensure!(
        name.as_bytes()[0].is_ascii_alphanumeric(),
        "feed name must start with a letter or digit"
    );
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub path: String,
    pub size: u64,
    /// BLAKE3 hash of the raw artifact bytes.
    pub hash: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub publisher: [u8; 32],
    pub feed: String,
    /// Strictly increasing per feed, starting at 1.
    pub sequence: u64,
    /// Publisher-claimed publication time, Unix seconds. Not proof of freshness.
    pub published_at: u64,
    pub content_type: String,
    /// Producer-defined schema/compatibility identifier; opaque to Speakeasy.
    pub schema: String,
    pub description: String,
    /// Producer-defined metadata (coverage, provenance, ...), sorted by key.
    pub attributes: Vec<(String, String)>,
    /// Sorted strictly ascending by path.
    pub artifacts: Vec<Artifact>,
}

impl Manifest {
    pub fn feed_id(&self) -> FeedId {
        FeedId::derive(&self.publisher, &self.feed)
    }

    pub fn total_size(&self) -> u64 {
        self.artifacts.iter().map(|a| a.size).sum()
    }

    pub fn validate(&self, limits: &Limits) -> Result<()> {
        validate_feed_name(&self.feed)?;
        ensure!(self.sequence >= 1, "sequence must be >= 1");
        for (name, s) in [
            ("content_type", &self.content_type),
            ("schema", &self.schema),
            ("description", &self.description),
        ] {
            ensure!(s.len() <= limits.max_text_len, "{name} too long");
        }
        ensure!(
            self.attributes.len() <= limits.max_attributes,
            "too many attributes"
        );
        let mut prev: Option<&str> = None;
        for (k, v) in &self.attributes {
            ensure!(!k.is_empty(), "empty attribute key");
            ensure!(
                k.len() <= limits.max_text_len && v.len() <= limits.max_text_len,
                "attribute too long"
            );
            if let Some(p) = prev {
                ensure!(p < k.as_str(), "attribute keys not strictly sorted");
            }
            prev = Some(k);
        }
        ensure!(
            self.artifacts.len() <= limits.max_artifacts,
            "too many artifacts ({} > {})",
            self.artifacts.len(),
            limits.max_artifacts
        );
        validate_path_set(self.artifacts.iter().map(|a| a.path.as_str()))?;
        let mut total: u64 = 0;
        for a in &self.artifacts {
            ensure!(
                a.size <= limits.max_artifact_size,
                "artifact {} exceeds size limit",
                a.path
            );
            total = total.checked_add(a.size).context("size overflow")?;
        }
        ensure!(total <= limits.max_total_size, "total size exceeds limit");
        Ok(())
    }

    /// The exact bytes covered by the publisher signature.
    pub fn signed_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(256 + self.artifacts.len() * 64);
        out.extend_from_slice(DOMAIN_TAG);
        out.extend_from_slice(&self.publisher);
        put_str(&mut out, &self.feed);
        out.extend_from_slice(&self.sequence.to_be_bytes());
        out.extend_from_slice(&self.published_at.to_be_bytes());
        put_str(&mut out, &self.content_type);
        put_str(&mut out, &self.schema);
        put_str(&mut out, &self.description);
        out.extend_from_slice(&(self.attributes.len() as u32).to_be_bytes());
        for (k, v) in &self.attributes {
            put_str(&mut out, k);
            put_str(&mut out, v);
        }
        out.extend_from_slice(&(self.artifacts.len() as u32).to_be_bytes());
        for a in &self.artifacts {
            put_str(&mut out, &a.path);
            out.extend_from_slice(&a.size.to_be_bytes());
            out.extend_from_slice(&a.hash);
        }
        out
    }

    fn decode_signed_bytes(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader { buf: bytes };
        ensure!(
            r.take(DOMAIN_TAG.len())? == DOMAIN_TAG,
            "not a speakeasy v1 manifest"
        );
        let publisher = r.array32()?;
        let feed = r.string()?;
        let sequence = r.u64()?;
        let published_at = r.u64()?;
        let content_type = r.string()?;
        let schema = r.string()?;
        let description = r.string()?;
        let n_attr = r.u32()? as usize;
        ensure!(n_attr <= r.buf.len() / 8, "attribute count exceeds data");
        let mut attributes = Vec::with_capacity(n_attr);
        for _ in 0..n_attr {
            attributes.push((r.string()?, r.string()?));
        }
        let n_art = r.u32()? as usize;
        ensure!(n_art <= r.buf.len() / 44, "artifact count exceeds data");
        let mut artifacts = Vec::with_capacity(n_art);
        for _ in 0..n_art {
            let path = r.string()?;
            let size = r.u64()?;
            let hash = r.array32()?;
            artifacts.push(Artifact { path, size, hash });
        }
        ensure!(r.buf.is_empty(), "trailing bytes in manifest");
        Ok(Self {
            publisher,
            feed,
            sequence,
            published_at,
            content_type,
            schema,
            description,
            attributes,
            artifacts,
        })
    }

    /// Validate and sign, producing the manifest envelope.
    pub fn sign(&self, key: &SigningKey, limits: &Limits) -> Result<SignedManifest> {
        ensure!(
            key.verifying_key().to_bytes() == self.publisher,
            "signing key does not match manifest publisher"
        );
        self.validate(limits)?;
        let mut bytes = self.signed_bytes();
        let sig: Signature = key.sign(&bytes);
        bytes.extend_from_slice(&sig.to_bytes());
        ensure!(
            bytes.len() <= limits.max_manifest_bytes,
            "manifest exceeds size limit"
        );
        Ok(SignedManifest {
            manifest: self.clone(),
            bytes,
        })
    }
}

/// A manifest together with its exact envelope bytes (`signed_bytes || signature`).
#[derive(Debug, Clone)]
pub struct SignedManifest {
    pub manifest: Manifest,
    pub bytes: Vec<u8>,
}

impl SignedManifest {
    /// Parse an envelope, check it against `expected_publisher`, verify the
    /// signature, and validate the content.
    pub fn verify(bytes: &[u8], expected_publisher: &[u8; 32], limits: &Limits) -> Result<Self> {
        ensure!(
            bytes.len() <= limits.max_manifest_bytes,
            "manifest exceeds size limit"
        );
        ensure!(bytes.len() > SIGNATURE_LEN, "manifest too short");
        let (signed, sig) = bytes.split_at(bytes.len() - SIGNATURE_LEN);
        let manifest = Manifest::decode_signed_bytes(signed)?;
        if &manifest.publisher != expected_publisher {
            bail!("manifest publisher does not match the pinned publisher key");
        }
        let key = VerifyingKey::from_bytes(expected_publisher).context("invalid publisher key")?;
        let sig = Signature::from_bytes(sig.try_into().expect("64 bytes"));
        key.verify_strict(signed, &sig)
            .map_err(|_| anyhow::anyhow!("invalid manifest signature"))?;
        manifest.validate(limits)?;
        Ok(Self {
            manifest,
            bytes: bytes.to_vec(),
        })
    }

    /// BLAKE3 of the envelope; identifies this exact signed manifest.
    pub fn manifest_hash(&self) -> [u8; 32] {
        *blake3::hash(&self.bytes).as_bytes()
    }
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u32).to_be_bytes());
    out.extend_from_slice(s.as_bytes());
}

struct Reader<'a> {
    buf: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        ensure!(self.buf.len() >= n, "truncated manifest");
        let (head, tail) = self.buf.split_at(n);
        self.buf = tail;
        Ok(head)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into()?))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into()?))
    }
    fn array32(&mut self) -> Result<[u8; 32]> {
        Ok(self.take(32)?.try_into()?)
    }
    fn string(&mut self) -> Result<String> {
        let n = self.u32()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).context("invalid UTF-8 in manifest")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn sample() -> Manifest {
        Manifest {
            publisher: key().verifying_key().to_bytes(),
            feed: "sample-events".into(),
            sequence: 3,
            published_at: 1_790_000_000,
            content_type: "application/octet-stream".into(),
            schema: "example.schema/1".into(),
            description: "Fictional fixture".into(),
            attributes: vec![("coverage".into(), "nowhere".into())],
            artifacts: vec![
                Artifact {
                    path: "a/b.bin".into(),
                    size: 3,
                    hash: *blake3::hash(b"abc").as_bytes(),
                },
                Artifact {
                    path: "c.txt".into(),
                    size: 0,
                    hash: *blake3::hash(b"").as_bytes(),
                },
            ],
        }
    }

    /// Golden vector: any change to the encoding or signing must change this
    /// test deliberately (and bump the format version).
    #[test]
    fn golden_vector() {
        const ENVELOPE_HEX: &str = concat!(
            "737065616b65617379206d616e69666573742076310aea4a6c63e29c520abef5507b132ec5f99547",
            "76aebebe7b92421eea691446d22c0000000d73616d706c652d6576656e7473000000000000000300",
            "0000006ab13b80000000186170706c69636174696f6e2f6f637465742d73747265616d0000001065",
            "78616d706c652e736368656d612f310000001146696374696f6e616c206669787475726500000001",
            "00000008636f766572616765000000076e6f77686572650000000200000007612f622e62696e0000",
            "0000000000036437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d850000",
            "0005632e7478740000000000000000af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc",
            "9a93cae41f3262363a1e5054738135acc6f7ddcf6367620f909cd4cf32041c9db2400d96d0d93726",
            "4e6565f756992d9d6818260f9363aa7de5fb836c98b511fada39b63f968304",
        );
        let signed = sample().sign(&key(), &Limits::default()).unwrap();
        assert_eq!(hex::encode(&signed.bytes), ENVELOPE_HEX);
        // Ed25519 signatures are deterministic, so the whole envelope is fixed.
        let parsed = SignedManifest::verify(
            &hex::decode(ENVELOPE_HEX).unwrap(),
            &sample().publisher,
            &Limits::default(),
        )
        .unwrap();
        assert_eq!(parsed.manifest, sample());
    }

    #[test]
    fn roundtrip_and_verify() {
        let m = sample();
        let signed = m.sign(&key(), &Limits::default()).unwrap();
        let v = SignedManifest::verify(&signed.bytes, &m.publisher, &Limits::default()).unwrap();
        assert_eq!(v.manifest, m);
    }

    #[test]
    fn rejects_tampering_anywhere() {
        let m = sample();
        let signed = m.sign(&key(), &Limits::default()).unwrap();
        for i in 0..signed.bytes.len() {
            let mut b = signed.bytes.clone();
            b[i] ^= 0x01;
            assert!(
                SignedManifest::verify(&b, &m.publisher, &Limits::default()).is_err(),
                "tampered byte {i} accepted"
            );
        }
        let mut b = signed.bytes.clone();
        b.push(0);
        assert!(SignedManifest::verify(&b, &m.publisher, &Limits::default()).is_err());
        assert!(
            SignedManifest::verify(
                &signed.bytes[..signed.bytes.len() - 1],
                &m.publisher,
                &Limits::default()
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_other_publisher() {
        let m = sample();
        let signed = m.sign(&key(), &Limits::default()).unwrap();
        let other = SigningKey::from_bytes(&[8u8; 32])
            .verifying_key()
            .to_bytes();
        assert!(SignedManifest::verify(&signed.bytes, &other, &Limits::default()).is_err());
        // A manifest claiming another publisher, signed by the wrong key.
        let mut forged = m.clone();
        forged.publisher = other;
        let mut bytes = forged.signed_bytes();
        bytes.extend_from_slice(&key().sign(&forged.signed_bytes()).to_bytes());
        assert!(SignedManifest::verify(&bytes, &other, &Limits::default()).is_err());
    }

    #[test]
    fn signed_but_unsafe_paths_are_rejected() {
        let mut m = sample();
        m.artifacts[0].path = "../escape".into();
        // Bypass `sign`'s validation to model a malicious but validly signed manifest.
        let mut bytes = m.signed_bytes();
        bytes.extend_from_slice(&key().sign(&m.signed_bytes()).to_bytes());
        let err = SignedManifest::verify(&bytes, &m.publisher, &Limits::default()).unwrap_err();
        assert!(err.to_string().contains(".."), "{err}");
    }

    #[test]
    fn enforces_limits() {
        let m = sample();
        let limits = Limits {
            max_artifact_size: 2,
            ..Limits::default()
        };
        assert!(m.sign(&key(), &limits).is_err());
        let signed = m.sign(&key(), &Limits::default()).unwrap();
        assert!(SignedManifest::verify(&signed.bytes, &m.publisher, &limits).is_err());
        let limits = Limits {
            max_manifest_bytes: 64,
            ..Limits::default()
        };
        assert!(SignedManifest::verify(&signed.bytes, &m.publisher, &limits).is_err());
    }

    #[test]
    fn artifact_hash_matches_iroh_blobs_hash() {
        let data = b"speakeasy artifact";
        assert_eq!(
            iroh_blobs::Hash::new(data).as_bytes(),
            blake3::hash(data).as_bytes()
        );
    }
}
