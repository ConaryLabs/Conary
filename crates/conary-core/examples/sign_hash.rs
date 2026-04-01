// conary-core/examples/sign_hash.rs
//
// CI signing helper: reads an Ed25519 seed from RELEASE_SIGNING_KEY,
// computes SHA-256 of a file, and prints a base64 Ed25519 signature.

use std::env;
use std::fs::File;
use std::io::Read;
use std::process;

use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};

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

    if key_hex.len() != 64 {
        eprintln!(
            "error: RELEASE_SIGNING_KEY must be exactly 64 hex characters (got {})",
            key_hex.len()
        );
        process::exit(1);
    }

    let seed_bytes: [u8; 32] = match hex::decode(&key_hex) {
        Ok(bytes) => bytes.try_into().unwrap_or_else(|v: Vec<u8>| {
            eprintln!("error: decoded key is {} bytes, expected 32", v.len());
            process::exit(1);
        }),
        Err(e) => {
            eprintln!("error: RELEASE_SIGNING_KEY is not valid hex: {e}");
            process::exit(1);
        }
    };

    let signing_key = SigningKey::from_bytes(&seed_bytes);

    let args: Vec<String> = env::args().collect();

    if args.iter().any(|a| a == "--show-public-key") {
        let public_key = signing_key.verifying_key();
        print!("{}", hex::encode(public_key.as_bytes()));
        return;
    }

    let file_path = match args.get(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: sign_hash [--show-public-key | <file>]");
            process::exit(1);
        }
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
