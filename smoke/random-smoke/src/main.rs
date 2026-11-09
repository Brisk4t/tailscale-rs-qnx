//! getrandom smoke test for x86_64-pc-qnx: checks each patched getrandom version returns
//! plausible randomness (from /dev/urandom on QNX). Not a statistical RNG test suite; it catches
//! "returns zeros / repeats / errors / short reads", which is what a broken port looks like.
//! Also runs known-answer and round-trip checks on ring, the TLS crypto provider on QNX.

use std::collections::HashSet;
use std::time::Instant;

type Fill = fn(&mut [u8]) -> Result<(), String>;

fn fill02(b: &mut [u8]) -> Result<(), String> {
    getrandom02::getrandom(b).map_err(|e| e.to_string())
}
fn fill03(b: &mut [u8]) -> Result<(), String> {
    getrandom03::fill(b).map_err(|e| e.to_string())
}
fn fill04(b: &mut [u8]) -> Result<(), String> {
    getrandom04::fill(b).map_err(|e| e.to_string())
}

fn check(fill: Fill) -> Result<String, String> {
    // Many small fills (the key-generation pattern): no two 32-byte values may repeat.
    let mut seen = HashSet::new();
    for _ in 0..10_000 {
        let mut k = [0u8; 32];
        fill(&mut k)?;
        if !seen.insert(k) {
            return Err("repeated 32-byte value".into());
        }
    }
    // One large fill: bit balance and byte histogram must look uniform.
    let mut big = vec![0u8; 1 << 20];
    let t = Instant::now();
    fill(&mut big)?;
    let elapsed = t.elapsed();
    let ones: u64 = big.iter().map(|b| u64::from(b.count_ones())).sum();
    let frac = ones as f64 / (big.len() * 8) as f64;
    if !(0.499..=0.501).contains(&frac) {
        return Err(format!("bit balance {frac:.5}"));
    }
    let mut hist = [0u64; 256];
    for &b in &big {
        hist[b as usize] += 1;
    }
    let expected = big.len() as f64 / 256.0;
    let chi2: f64 = hist.iter().map(|&c| (c as f64 - expected).powi(2) / expected).sum();
    // 255 degrees of freedom: mean 255, sd ~22.6. Reject only wildly non-uniform output.
    if !(150.0..=400.0).contains(&chi2) {
        return Err(format!("byte histogram chi^2 = {chi2:.1}"));
    }
    // Empty and odd-sized fills must work too.
    fill(&mut [])?;
    fill(&mut [0u8; 7])?;
    Ok(format!(
        "10k unique keys, 1 MiB in {elapsed:?}, ones={frac:.5}, chi2={chi2:.1}"
    ))
}

/// ring on QNX: its RNG (via getrandom 0.2) and the primitives TLS 1.3 uses, on ring's
/// portable-C path (ring builds no assembly for target_os = "qnx").
fn ring_checks() -> Result<String, String> {
    use ring::{aead, agreement, digest, rand, signature};
    fn unspecified<E>(what: &'static str) -> impl FnOnce(E) -> String {
        move |_| format!("{what} failed")
    }
    let rng = rand::SystemRandom::new();

    let mut a = [0u8; 32];
    let mut b = [0u8; 32];
    rand::SecureRandom::fill(&rng, &mut a).map_err(unspecified("SystemRandom::fill"))?;
    rand::SecureRandom::fill(&rng, &mut b).map_err(unspecified("SystemRandom::fill"))?;
    if a == b || a == [0; 32] {
        return Err("SystemRandom returned repeated/zero output".into());
    }

    // SHA-256 known answer ("abc", FIPS 180-2).
    let d = digest::digest(&digest::SHA256, b"abc");
    let want = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    let got: String = d.as_ref().iter().map(|x| format!("{x:02x}")).collect();
    if got != want {
        return Err(format!("SHA-256(abc) = {got}"));
    }

    // X25519: both sides derive the same secret.
    let sk1 = agreement::EphemeralPrivateKey::generate(&agreement::X25519, &rng)
        .map_err(unspecified("X25519 keygen"))?;
    let sk2 = agreement::EphemeralPrivateKey::generate(&agreement::X25519, &rng)
        .map_err(unspecified("X25519 keygen"))?;
    let pk1 = sk1.compute_public_key().map_err(unspecified("X25519 pubkey"))?;
    let pk2 = sk2.compute_public_key().map_err(unspecified("X25519 pubkey"))?;
    let s1 = agreement::agree_ephemeral(sk1, &agreement::UnparsedPublicKey::new(&agreement::X25519, pk2), |k| k.to_vec())
        .map_err(unspecified("X25519 agree"))?;
    let s2 = agreement::agree_ephemeral(sk2, &agreement::UnparsedPublicKey::new(&agreement::X25519, pk1), |k| k.to_vec())
        .map_err(unspecified("X25519 agree"))?;
    if s1 != s2 {
        return Err("X25519 shared secrets differ".into());
    }

    // AEADs used by TLS 1.3: seal/open round trip, and a flipped bit must be rejected.
    for alg in [&aead::AES_256_GCM, &aead::CHACHA20_POLY1305] {
        let mut key = vec![0u8; alg.key_len()];
        rand::SecureRandom::fill(&rng, &mut key).map_err(unspecified("key fill"))?;
        let key = aead::LessSafeKey::new(aead::UnboundKey::new(alg, &key).map_err(unspecified("AEAD key"))?);
        let msg = b"hello from QNX over TLS".to_vec();
        let mut buf = msg.clone();
        let nonce = || aead::Nonce::assume_unique_for_key([7u8; 12]);
        key.seal_in_place_append_tag(nonce(), aead::Aad::from(b"hdr"), &mut buf)
            .map_err(unspecified("AEAD seal"))?;
        let mut tampered = buf.clone();
        let opened = key.open_in_place(nonce(), aead::Aad::from(b"hdr"), &mut buf)
            .map_err(unspecified("AEAD open"))?;
        if opened != msg.as_slice() {
            return Err(format!("{alg:?} round trip mismatch"));
        }
        tampered[0] ^= 1;
        if key.open_in_place(nonce(), aead::Aad::from(b"hdr"), &mut tampered).is_ok() {
            return Err(format!("{alg:?} accepted a tampered ciphertext"));
        }
    }

    // Ed25519 sign/verify, and a wrong message must fail.
    let pkcs8 = signature::Ed25519KeyPair::generate_pkcs8(&rng).map_err(unspecified("Ed25519 keygen"))?;
    let kp = signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).map_err(unspecified("Ed25519 parse"))?;
    let sig = kp.sign(b"msg");
    let pk = signature::UnparsedPublicKey::new(&signature::ED25519, signature::KeyPair::public_key(&kp));
    pk.verify(b"msg", sig.as_ref()).map_err(unspecified("Ed25519 verify"))?;
    if pk.verify(b"other", sig.as_ref()).is_ok() {
        return Err("Ed25519 verified a wrong message".into());
    }
    Ok("SystemRandom, SHA-256 KAT, X25519, AES-256-GCM, ChaCha20-Poly1305, Ed25519".into())
}

/// Deterministic ring outputs (fixed keys/nonces/inputs), summarised as one digest per
/// primitive. Run on a trusted host and on QNX: identical digests mean QNX's ring (including its
/// assembly paths) computes the same bytes. Lengths span the scalar, SIMD and bulk/AVX2 code paths.
fn ring_vectors() -> Vec<(&'static str, String)> {
    use ring::{aead, digest, hmac, signature};
    const LENS: &[usize] = &[0, 1, 15, 16, 17, 31, 32, 63, 64, 65, 127, 128, 255, 256, 511, 1024, 4095, 16384, 65537];
    let input = |n: usize| (0..n).map(|i| (i.wrapping_mul(31) ^ (i >> 7)) as u8).collect::<Vec<u8>>();
    let hex = |d: digest::Digest| d.as_ref().iter().map(|x| format!("{x:02x}")).collect::<String>();
    let mut out = Vec::new();

    for (name, alg) in [("sha256", &digest::SHA256), ("sha384", &digest::SHA384), ("sha512", &digest::SHA512)] {
        let mut ctx = digest::Context::new(&digest::SHA256);
        for &n in LENS {
            ctx.update(digest::digest(alg, &input(n)).as_ref());
        }
        out.push((name, hex(ctx.finish())));
    }

    let key = hmac::Key::new(hmac::HMAC_SHA256, b"fixed hmac key for qnx vectors");
    let mut ctx = digest::Context::new(&digest::SHA256);
    for &n in LENS {
        ctx.update(hmac::sign(&key, &input(n)).as_ref());
    }
    out.push(("hmac-sha256", hex(ctx.finish())));

    for (name, alg) in [
        ("aes-128-gcm", &aead::AES_128_GCM),
        ("aes-256-gcm", &aead::AES_256_GCM),
        ("chacha20-poly1305", &aead::CHACHA20_POLY1305),
    ] {
        let key_bytes: Vec<u8> = (0..alg.key_len() as u8).collect();
        let key = aead::LessSafeKey::new(aead::UnboundKey::new(alg, &key_bytes).unwrap());
        let mut ctx = digest::Context::new(&digest::SHA256);
        for (i, &n) in LENS.iter().enumerate() {
            let mut nonce = [0u8; 12];
            nonce[11] = i as u8;
            let mut buf = input(n);
            let aad = input(i * 3);
            key.seal_in_place_append_tag(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(&aad),
                &mut buf,
            )
            .unwrap();
            ctx.update(&buf);
            let opened = key
                .open_in_place(aead::Nonce::assume_unique_for_key(nonce), aead::Aad::from(&aad), &mut buf)
                .expect("open of own ciphertext failed");
            assert_eq!(opened, input(n).as_slice());
        }
        out.push((name, hex(ctx.finish())));
    }

    // Ed25519 signatures are deterministic for a fixed seed.
    let kp = signature::Ed25519KeyPair::from_seed_unchecked(&[42u8; 32]).unwrap();
    let mut ctx = digest::Context::new(&digest::SHA256);
    ctx.update(signature::KeyPair::public_key(&kp).as_ref());
    for &n in LENS {
        ctx.update(kp.sign(&input(n)).as_ref());
    }
    out.push(("ed25519", hex(ctx.finish())));
    out
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("ring-vectors") {
        for (name, digest) in ring_vectors() {
            println!("{name:<18} {digest}");
        }
        return;
    }
    let mut failed = 0;
    for (name, fill) in [("0.2.17", fill02 as Fill), ("0.3.4", fill03), ("0.4.3", fill04)] {
        // Also exercise it from several threads at once.
        let threads: Vec<_> = (0..4).map(|_| std::thread::spawn(move || check(fill))).collect();
        let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        match results.iter().find(|r| r.is_err()) {
            None => println!("PASS getrandom {name:<7} {} (x4 threads)", results[0].as_ref().unwrap()),
            Some(Err(e)) => {
                failed += 1;
                println!("FAIL getrandom {name:<7} {e}");
            }
            Some(Ok(_)) => unreachable!(),
        }
    }
    match ring_checks() {
        Ok(msg) => println!("PASS ring 0.17.14   {msg}"),
        Err(e) => {
            failed += 1;
            println!("FAIL ring 0.17.14   {e}");
        }
    }
    println!("4 checks, {failed} failures");
    std::process::exit(if failed == 0 { 0 } else { 1 });
}
