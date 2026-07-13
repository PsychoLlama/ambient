//! Agreement test: the deploy layer's private `classify_name` and the
//! engine's public `disk_store::classify_binding` must classify every
//! (previous, next) binding pair identically. The two exist separately only
//! because the engine cannot depend on this crate (it is upstream); their
//! semantics must never diverge.

use std::sync::Arc;

use ambient_engine::disk_store::{BindingChange, classify_binding};

use super::{Binding, NameChange, classify_name};

/// Map the two enums onto a common shape for comparison.
fn as_pair(deploy: NameChange, engine: BindingChange) -> (u8, u8) {
    let d = match deploy {
        NameChange::Unchanged => 0,
        NameChange::Rebound => 1,
        NameChange::Retired => 2,
        NameChange::Added => 3,
    };
    let e = match engine {
        BindingChange::Unchanged => 0,
        BindingChange::Rebound => 1,
        BindingChange::Retired => 2,
        BindingChange::Added => 3,
    };
    (d, e)
}

fn binding(hash: blake3::Hash, sig: Option<&str>) -> Binding {
    Binding {
        hash,
        signature: sig.map(Arc::from),
    }
}

#[test]
fn classify_name_and_classify_binding_agree_across_the_matrix() {
    let h1 = blake3::hash(b"one");
    let h2 = blake3::hash(b"two");
    let hashes = [h1, h2];
    let sigs = [None, Some("A"), Some("B")];

    // Every next binding, against every prev binding *and* against no prev.
    for &next_hash in &hashes {
        for &next_sig in &sigs {
            let next = binding(next_hash, next_sig);

            // prev = None (a fresh name).
            let deploy = classify_name(None, &next);
            let engine = classify_binding(None, (next.hash.as_bytes(), next.signature.as_deref()));
            let (d, e) = as_pair(deploy, engine);
            assert_eq!(
                d, e,
                "disagreement on a fresh name: {deploy:?} vs {engine:?}"
            );

            for &prev_hash in &hashes {
                for &prev_sig in &sigs {
                    let prev = binding(prev_hash, prev_sig);
                    let deploy = classify_name(Some(&prev), &next);
                    let engine = classify_binding(
                        Some((prev.hash.as_bytes(), prev.signature.as_deref())),
                        (next.hash.as_bytes(), next.signature.as_deref()),
                    );
                    let (d, e) = as_pair(deploy, engine);
                    assert_eq!(
                        d, e,
                        "disagreement: prev=({prev_hash:?},{prev_sig:?}) \
                         next=({next_hash:?},{next_sig:?}): {deploy:?} vs {engine:?}"
                    );
                }
            }
        }
    }
}
