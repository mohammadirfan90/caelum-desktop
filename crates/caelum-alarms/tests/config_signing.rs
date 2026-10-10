//! Signed configuration bundles (F-SYS-07, CR-012 §2): same keys and trust policy as the ruleset,
//! a separate signing domain per kind, and every failure rejects the bundle.

use caelum_alarms::signing::{
    digest, sign, sign_config, verify_config, ConfigVerifyError, KeyClass, SignerRole, TrustPolicy,
    TrustedKey,
};
use ed25519_dalek::SigningKey;

const BUNDLE: &[u8] = b"[roster]\nversion = \"1\"\n";

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn trusted(k: &SigningKey, class: KeyClass) -> TrustedKey {
    TrustedKey {
        class,
        key: k.verifying_key(),
    }
}

fn envelope(kind: &str, sha: &str, version: &str, entries: &[(String, String, &str)]) -> String {
    let mut s = format!("kind = \"{kind}\"\nversion = \"{version}\"\nsha256 = \"{sha}\"\n");
    for (key_id, sig, role) in entries {
        s.push_str(&format!(
            "\n[[signature]]\nrole = \"{role}\"\nkey_id = \"{key_id}\"\nsignature = \"{sig}\"\n"
        ));
    }
    s
}

fn signed(kind: &str, bytes: &[u8], version: &str, k: &SigningKey) -> String {
    let e = sign_config(kind, bytes, version, k);
    envelope(
        kind,
        &digest(bytes),
        version,
        &[(e.key_id, e.signature, "author")],
    )
}

#[test]
fn f_sys_07_a_signed_bundle_verifies_and_reports_its_key_class() {
    let k = key(1);
    let env = signed("crew", BUNDLE, "1", &k);
    let dev = verify_config(
        "crew",
        BUNDLE,
        &env,
        &[trusted(&k, KeyClass::Development)],
        TrustPolicy::Development,
    );
    assert_eq!(
        dev.as_ref().map(|v| (v.version.as_str(), v.development)),
        Ok(("1", true))
    );
    assert_eq!(dev.map(|v| v.sha256), Ok(digest(BUNDLE)));
    let flight = verify_config(
        "crew",
        BUNDLE,
        &env,
        &[trusted(&k, KeyClass::Flight)],
        TrustPolicy::Flight,
    );
    assert_eq!(flight.map(|v| v.development), Ok(false));
}

#[test]
fn f_sys_07_release_builds_accept_flight_keys_only() {
    let k = key(1);
    let env = signed("crew", BUNDLE, "1", &k);
    let r = verify_config(
        "crew",
        BUNDLE,
        &env,
        &[trusted(&k, KeyClass::Development)],
        TrustPolicy::Flight,
    );
    assert!(matches!(r, Err(ConfigVerifyError::UntrustedKey(_))));
}

#[test]
fn f_sys_07_tampered_wrong_kind_and_unsigned_bundles_are_rejected() {
    let k = key(1);
    let keys = [trusted(&k, KeyClass::Development)];
    let dev = TrustPolicy::Development;
    let env = signed("crew", BUNDLE, "1", &k);

    assert_eq!(
        verify_config("crew", b"[roster]\nversion = \"2\"\n", &env, &keys, dev),
        Err(ConfigVerifyError::DigestMismatch)
    );
    assert_eq!(
        verify_config("channels", BUNDLE, &env, &keys, dev),
        Err(ConfigVerifyError::WrongKind {
            expected: "channels".into(),
            found: "crew".into()
        })
    );
    // Same bytes and digest, but signed as another kind: the domain separation refuses it.
    let other = sign_config("channels", BUNDLE, "1", &k);
    let relabelled = envelope(
        "crew",
        &digest(BUNDLE),
        "1",
        &[(other.key_id, other.signature, "author")],
    );
    assert!(matches!(
        verify_config("crew", BUNDLE, &relabelled, &keys, dev),
        Err(ConfigVerifyError::BadSignature(_))
    ));
    // A ruleset signature over the same bytes never passes as a configuration signature.
    let rs = sign(BUNDLE, "1", SignerRole::Author, &k);
    let ruleset_sig = envelope(
        "crew",
        &digest(BUNDLE),
        "1",
        &[(rs.key_id, rs.signature, "author")],
    );
    assert!(matches!(
        verify_config("crew", BUNDLE, &ruleset_sig, &keys, dev),
        Err(ConfigVerifyError::BadSignature(_))
    ));
    // The version is signed too.
    let e = sign_config("crew", BUNDLE, "1", &k);
    let bumped = envelope(
        "crew",
        &digest(BUNDLE),
        "2",
        &[(e.key_id.clone(), e.signature.clone(), "author")],
    );
    assert!(matches!(
        verify_config("crew", BUNDLE, &bumped, &keys, dev),
        Err(ConfigVerifyError::BadSignature(_))
    ));
    assert_eq!(
        verify_config(
            "crew",
            BUNDLE,
            &envelope("crew", &digest(BUNDLE), "1", &[]),
            &keys,
            dev
        ),
        Err(ConfigVerifyError::Unsigned)
    );
    let malformed = envelope(
        "crew",
        &digest(BUNDLE),
        "1",
        &[(e.key_id.clone(), "zz".into(), "author")],
    );
    assert!(matches!(
        verify_config("crew", BUNDLE, &malformed, &keys, dev),
        Err(ConfigVerifyError::BadSignature(_))
    ));
    assert!(matches!(
        verify_config(
            "crew",
            BUNDLE,
            &signed("crew", BUNDLE, "1", &key(2)),
            &keys,
            dev
        ),
        Err(ConfigVerifyError::UntrustedKey(_))
    ));
    assert!(matches!(
        verify_config("crew", BUNDLE, "kind = 1", &keys, dev),
        Err(ConfigVerifyError::Envelope(_))
    ));
    assert!(matches!(
        verify_config("crew", BUNDLE, &format!("{env}extra = 1\n"), &keys, dev),
        Err(ConfigVerifyError::Envelope(_))
    ));
}

#[test]
fn f_sys_07_a_bundle_needs_an_author_signature() {
    let k = key(1);
    let e = sign_config("crew", BUNDLE, "1", &k);
    let env = envelope(
        "crew",
        &digest(BUNDLE),
        "1",
        &[(e.key_id, e.signature, "medical_approver")],
    );
    // The role is not part of the signed message, so the signature verifies; the role check
    // still refuses a bundle without an author.
    assert_eq!(
        verify_config(
            "crew",
            BUNDLE,
            &env,
            &[trusted(&k, KeyClass::Development)],
            TrustPolicy::Development
        ),
        Err(ConfigVerifyError::NoAuthor)
    );
}
