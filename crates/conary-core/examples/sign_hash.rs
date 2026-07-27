// conary-core/examples/sign_hash.rs
//
// CI signing helper: reads an Ed25519 seed from RELEASE_SIGNING_KEY,
// materializes the matching CCS release authority, or signs a file hash.

use std::env;
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;
use std::process;

use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};

fn signing_key_from_hex(key_hex: &str) -> Result<SigningKey, String> {
    if key_hex.len() != 64 {
        return Err(format!(
            "RELEASE_SIGNING_KEY must be exactly 64 hex characters (got {})",
            key_hex.len()
        ));
    }

    let seed_bytes: [u8; 32] = hex::decode(key_hex)
        .map_err(|error| format!("RELEASE_SIGNING_KEY is not valid hex: {error}"))?
        .try_into()
        .map_err(|bytes: Vec<u8>| format!("decoded key is {} bytes, expected 32", bytes.len()))?;
    Ok(SigningKey::from_bytes(&seed_bytes))
}

fn write_ccs_authority(directory: &Path, signing_key: SigningKey) -> Result<(), String> {
    let metadata = fs::symlink_metadata(directory)
        .map_err(|error| format!("cannot inspect authority directory: {error}"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("CCS authority destination must be a real directory".to_string());
    }

    let private_path = directory.join("release.private");
    let public_path = directory.join("release.public");
    let policy_path = directory.join("trust-policy.toml");
    for path in [&private_path, &public_path, &policy_path] {
        if path.exists() {
            return Err(format!(
                "refusing to replace existing CCS authority file {}",
                path.display()
            ));
        }
    }

    let keypair = conary_core::ccs::SigningKeyPair::from_signing_key(signing_key)
        .with_key_id("conary-release");
    let public_key = keypair.public_key_base64();
    keypair
        .save_to_files(&private_path, &public_path)
        .map_err(|error| format!("cannot write CCS release key pair: {error}"))?;
    fs::write(
        &policy_path,
        format!("trusted_keys = [\"{public_key}\"]\nrequire_timestamp = true\n"),
    )
    .map_err(|error| format!("cannot write CCS release trust policy: {error}"))?;
    Ok(())
}

fn usage() -> ! {
    eprintln!("usage: sign_hash [--show-public-key | --write-ccs-authority <directory> | <file>]");
    process::exit(1);
}

fn main() {
    let key_hex = match env::var("RELEASE_SIGNING_KEY") {
        Ok(v) if !v.is_empty() => v,
        Ok(_) => {
            eprintln!("error: RELEASE_SIGNING_KEY is empty");
            process::exit(1);
        }
        Err(_) => {
            eprintln!("error: RELEASE_SIGNING_KEY environment variable not set");
            process::exit(1);
        }
    };

    let signing_key = match signing_key_from_hex(&key_hex) {
        Ok(key) => key,
        Err(error) => {
            eprintln!("error: {error}");
            process::exit(1);
        }
    };

    let args: Vec<String> = env::args().skip(1).collect();
    let file_path = match args.as_slice() {
        [flag] if flag == "--show-public-key" => {
            print!("{}", hex::encode(signing_key.verifying_key().as_bytes()));
            return;
        }
        [flag, directory] if flag == "--write-ccs-authority" => {
            if let Err(error) = write_ccs_authority(Path::new(directory), signing_key) {
                eprintln!("error: {error}");
                process::exit(1);
            }
            return;
        }
        [file_path] => file_path,
        _ => usage(),
    };

    let mut file = match File::open(file_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error: cannot open {file_path}: {e}");
            process::exit(1);
        }
    };

    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                eprintln!("error: reading {file_path}: {e}");
                process::exit(1);
            }
        };
        hasher.update(&buf[..n]);
    }

    let hash_hex = hex::encode(hasher.finalize());
    let signature = signing_key.sign(hash_hex.as_bytes());
    print!("{}", BASE64.encode(signature.to_bytes()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ccs_authority_uses_the_exact_release_seed_and_private_mode() {
        let directory = tempfile::tempdir().unwrap();
        let signing_key = SigningKey::from_bytes(&[42; 32]);
        let expected_public = signing_key.verifying_key();

        write_ccs_authority(directory.path(), signing_key).unwrap();

        let private_path = directory.path().join("release.private");
        let loaded = conary_core::ccs::SigningKeyPair::load_from_file(&private_path).unwrap();
        assert_eq!(loaded.verifying_key(), expected_public);
        assert_eq!(loaded.key_id(), Some("conary-release"));

        let policy =
            conary_core::ccs::TrustPolicy::from_file(&directory.path().join("trust-policy.toml"))
                .unwrap();
        assert_eq!(policy.trusted_keys(), &[loaded.public_key_base64()]);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(private_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }
}
