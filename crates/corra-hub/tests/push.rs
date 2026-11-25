use std::io::Write;

use corra_hub::gitproxy::{MAX_PUSH_BYTES, approval_key, gunzip, is_guarded, push_commands};

const ZERO: &str = "0000000000000000000000000000000000000000";

fn body(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/pushes/{name}.bin",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn refs(name: &str) -> Vec<(String, String, String)> {
    push_commands(&body(name)).unwrap_or_else(|| panic!("{name} didn't parse"))
}

fn one(old: &str, new: &str, r: &str) -> Vec<(String, String, String)> {
    vec![(old.into(), new.into(), r.into())]
}

#[test]
fn real_pushes_parse_to_their_commands() {
    assert_eq!(
        refs("branch"),
        one(
            ZERO,
            "785b53f79b3121e6323dbd77c9d737126ccee593",
            "refs/heads/feature"
        )
    );
    assert_eq!(
        refs("main"),
        one(
            "c03375f7cbd4eaf0d66df7d4ad020a6c0e89a355",
            "3c1fddd6d39dd59a99832adbab34fc79347d7169",
            "refs/heads/main"
        )
    );
    assert_eq!(
        refs("tag"),
        one(
            ZERO,
            "3c1fddd6d39dd59a99832adbab34fc79347d7169",
            "refs/tags/v1"
        )
    );
    assert_eq!(
        refs("delete"),
        one(
            "785b53f79b3121e6323dbd77c9d737126ccee593",
            ZERO,
            "refs/heads/feature"
        )
    );
    assert_eq!(
        refs("force"),
        one(
            "85ab14dbf73071f3fe766d9a831ac4238e862b17",
            "fb23653db5027107444b27367665fda9390f2b43",
            "refs/heads/main"
        )
    );
    assert_eq!(
        refs("shallow"),
        one(
            ZERO,
            "e0ae86bb1908a0d6c9c94afce2f06e7ac197ae70",
            "refs/heads/fromshallow"
        )
    );
    let several: Vec<String> = refs("several").into_iter().map(|c| c.2).collect();
    assert_eq!(
        several,
        ["refs/heads/main", "refs/heads/one", "refs/heads/two"]
    );
}

#[test]
fn the_default_branch_and_tags_are_guarded_and_others_are_not() {
    let guarded = |name: &str| -> Vec<bool> {
        refs(name)
            .iter()
            .map(|(_, _, r)| is_guarded(r, "main"))
            .collect()
    };
    assert_eq!(guarded("main-first"), [true]);
    assert_eq!(guarded("main"), [true]);
    assert_eq!(guarded("force"), [true]);
    assert_eq!(guarded("tag"), [true]);
    assert_eq!(guarded("branch"), [false]);
    assert_eq!(guarded("delete"), [false]);
    assert_eq!(guarded("shallow"), [false]);
    assert_eq!(guarded("several"), [true, false, false]);
}

#[test]
fn garbage_and_truncated_pushes_are_refused() {
    assert!(push_commands(b"").is_none());
    assert!(push_commands(b"zzzz").is_none());
    assert!(push_commands(b"0010not a command").is_none());
    let main = body("main");
    let commands_end = main.windows(4).position(|w| w == b"0000").unwrap();
    assert!(push_commands(&main[..commands_end - 10]).is_none());
    let mut bad = main.clone();
    bad[8] = b'g';
    assert!(push_commands(&bad).is_none());
}

#[test]
fn a_gzipped_push_unpacks_to_the_same_commands() {
    let raw = body("several");
    let mut gz = flate2::write::GzEncoder::new(vec![], flate2::Compression::default());
    gz.write_all(&raw).unwrap();
    let packed = gz.finish().unwrap();
    let unpacked = gunzip(&packed, MAX_PUSH_BYTES).unwrap().unwrap();
    assert_eq!(push_commands(&unpacked), push_commands(&raw));
}

#[test]
fn a_push_that_unpacks_past_the_limit_is_refused() {
    let mut gz = flate2::write::GzEncoder::new(vec![], flate2::Compression::best());
    gz.write_all(&vec![0u8; 8 << 20]).unwrap();
    let bomb = gz.finish().unwrap();
    assert!(bomb.len() < 64 << 10);
    assert!(gunzip(&bomb, 1 << 20).unwrap().is_none());
    assert_eq!(gunzip(&bomb, 8 << 20).unwrap().unwrap().len(), 8 << 20);
    assert!(gunzip(b"not gzip", 1 << 20).is_err());
}

#[test]
fn an_approval_covers_only_the_exact_push() {
    let approved = refs("main");
    let key = approval_key("owner/repo", &approved, "main");
    let (old, new, r) = approved[0].clone();
    let same_prefix = format!("{}{}", &new[..8], "f".repeat(32));
    assert_ne!(
        key,
        approval_key("owner/repo", &one(&old, &same_prefix, &r), "main")
    );
    let other_base = "1".repeat(40);
    assert_ne!(
        key,
        approval_key("owner/repo", &one(&other_base, &new, &r), "main")
    );
    assert_ne!(key, approval_key("owner/other", &approved, "main"));
    assert_eq!(key, approval_key("OWNER/Repo", &approved, "main"));
    let mut with_branch = approved.clone();
    with_branch.push((ZERO.into(), new.clone(), "refs/heads/feature".into()));
    assert_eq!(key, approval_key("owner/repo", &with_branch, "main"));
}
