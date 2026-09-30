//! Secret key files. The publisher signing key (Ed25519, signs manifests) and
//! the node key (iroh endpoint identity, authenticates transport connections)
//! are distinct: whoever serves bytes need not be the publisher.

use std::{
    fs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
};

use anyhow::{Context, Result, ensure};

pub fn random_secret() -> Result<[u8; 32]> {
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).map_err(|e| anyhow::anyhow!("OS randomness unavailable: {e}"))?;
    Ok(b)
}

/// Create a new secret key file (hex, mode 0600). Fails if it already exists.
pub fn create_secret_file(path: &Path, secret: &[u8; 32]) -> Result<()> {
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("create key file {}", path.display()))?;
    writeln!(f, "{}", hex::encode(secret))?;
    f.sync_all()?;
    Ok(())
}

pub fn read_secret_file(path: &Path) -> Result<[u8; 32]> {
    let meta = fs::metadata(path).with_context(|| format!("read key file {}", path.display()))?;
    ensure!(
        meta.permissions().mode() & 0o077 == 0,
        "key file {} is accessible by other users; chmod 600 it",
        path.display()
    );
    let text = fs::read_to_string(path)?;
    let bytes = hex::decode(text.trim()).context("key file is not hex")?;
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("key file {} must hold 32 bytes", path.display()))
}
